//! `benilla-extract` — Phase 1 CLI over WoW 1.12.1 (build 5875) MPQ archives.
//!
//! Implemented: `list`, `extract` (raw bytes), operating on the **patch chain**
//! (later archives override earlier; real names come from patch.MPQ's `(listfile)`).
//! BLP->PNG and DBC->CSV come next.
//!
//! This file is the stable face: the clap [`Cli`]/[`Command`] definitions, the dispatch `match`,
//! and the tiny helpers shared across concerns. The heavier subcommands live in sibling modules by
//! concern: per-model M2 dump printers ([`m2dump`]), corpus-scan reports ([`scan`]), and the spell
//! visual chain ([`spellvis`]).

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
        /// Also write every authored mip level (`<stem>.mip<N>.png`) and print a per-level
        /// texel census: how many texels the author left transparent vs not, the luma range of
        /// each class, and how many sit below 128 — the set that DARKENS under a Mod2x lane,
        /// which reads no alpha. The "what does the far sampler see" instrument.
        #[arg(long)]
        mips: bool,
    },
    /// Composite ONE character's body atlas off the chain and report what painted what: the
    /// equipment blits in blit order (with the file each region name resolved to, or `MISSING`),
    /// the per-tile rows repainted vs the same character naked, and the geoset set the same
    /// equipment selects. The "this garment stops early / that slot repainted this one" instrument
    /// — a dressed body is ten fixed atlas tiles, and every defect in that class is one tile
    /// receiving the wrong contribution.
    Charatlas {
        /// `ChrRaces` id (1 human · 2 orc · 3 dwarf · 4 night elf · 5 undead · 6 tauren · 7 gnome ·
        /// 8 troll).
        #[arg(long)]
        race: u8,
        /// 0 male · 1 female.
        #[arg(long)]
        sex: u8,
        /// `skinColor` — the CharSections variation the base skin + head sections key on.
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
        /// The eight worn `ItemDisplayInfo` ids, comma-separated, in bodyslot order:
        /// shirt,chest,belt,pants,boots,wrist,gloves,tabard. `0` leaves a slot empty; a short list
        /// leaves the rest empty.
        #[arg(long, value_delimiter = ',', default_value = "0")]
        slots: Vec<u32>,
        /// The wearer's guild tabard, comma-separated in
        /// `emblemStyle,emblemColor,borderStyle,borderColor,backgroundColor` order — the five
        /// indices `SMSG_GUILD_QUERY_RESPONSE` carries. Omit for "no guild". Only
        /// paints when the tabard slot holds a guild-emblem display (20621 is the shipped one).
        #[arg(long, value_delimiter = ',')]
        emblem: Option<Vec<i32>>,
        /// Write the composited atlas here as a PNG (256²).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Measure every shipped glue scene's **art extent**: how far the opaque art
    /// of each `UI_*` diorama covers around its authored camera 0, and the window aspects past
    /// which the glue framing law stops widening and zooms instead. The instrument behind B330
    /// (the login backdrop's edges showing at 16:9) — the same measurement the client makes at
    /// scene spawn, printed for all seven scenes with the looser readings beside it.
    Glueextent {
        /// Also print every batch's footprint in the frame (front/back/clipped triangle counts,
        /// projected extents) — which card sets the measured edge.
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
    /// Dump a spell's visual chain: spell → SpellVisual stages → each kit's anim/sound (decision
    /// 0099's phase-2 instrument; columns per decision 0107).
    Spellvis {
        /// The `Spell.dbc` id (e.g. 133 = Fireball).
        spell_id: u32,
    },
    /// Census the **beam/chain** system: the 18-row `SpellChainEffects` table,
    /// then every `SpellVisualKit` that draws a beam — the row it names, its beam count and
    /// flag, and the spells that reach it through which lifecycle stage. The scope instrument
    /// for B161 ("Chain Lightning has no chain effect"), and the check that the `CharParamZero`
    /// small-int decode is the real mechanism: every live slot must land on a real row.
    Chaincensus,
    /// Census the **camera-shake** system: the 24 shipped `CameraShakes.dbc`
    /// presets and every `CreatureModelData` row that names one, through the footstep column or
    /// the death-thud column. The scope instrument for B298 ("walking past an Ancient Protector
    /// shakes no screen"), and the check that fields 11/12 really are `CameraShakes` keys — every
    /// live value must land on a real row.
    Shakecensus,
    /// Census the **death thud** — the body-fall sound a corpse makes on landing (`$DTH` →
    /// `0x6236e0`, the sibling of the camera shake above). Two halves: the `DeathThudLookups.dbc`
    /// matrix in full (`SizeClass × TerrainTypeSoundID` → the named land/water `SoundEntries`
    /// kits), and the population sweep of which creature M2s key a `$DTH` at all, with the size
    /// class their displays resolve to. The scope instrument for "a big corpse hits the ground
    /// silently": it separates authored silence (an empty water column) from a real gap.
    Thudcensus,
    /// Census the `SpellVisualKit` **CharProc** columns (a kit's effect on the BODY — its alpha,
    /// its tint): which proc types the shipped table carries, which lifecycle stage reaches each
    /// from a live spell, and every state-stage (aura-lifetime) proc in full. The scope instrument
    /// for the aura-state CharProc system.
    Charprocs,
    /// Census the `SpellVisualKit` **animation** column (field 2) — the half of a kit that plays a
    /// clip on the unit's own BODY, as opposed to its attach-point effect models, its CharProcs or
    /// its camera shake. Which anim ids the shipped table asks for, which lifecycle stage reaches
    /// each (the stage picks the consumer: a `cast`/`impact` anim is a one-shot on the
    /// caster/victim, a `state` anim belongs to an aura's whole life), then the state set in full
    /// and the impact set ranked. The scope instrument for "the spell landed on me and my
    /// character did nothing" — an anim that is never asked for leaves no trace to grep, unlike an
    /// effect model that fails to spawn. `ANIM-ONLY` marks the state kits whose whole visual is the
    /// anim, the class a "does this kit do anything?" test drops (the B114 shape one level over).
    Kitanim,
    /// Dump an M2's collision hull as the mover collides with it: vertex/triangle counts, the
    /// model-space AABB (WoW axes, Z up), and its extents — the "what does walking into this
    /// actually hit" instrument (the step-up climb-vs-slide asset question, decision 0195; a
    /// placement's own scale still multiplies these numbers).
    M2coll {
        /// Internal path to the `.m2` (forward or back slashes accepted).
        internal_path: String,
    },
    /// Dump an M2's animation sequences in file order: `AnimationData.dbc` id, loop/clamp flag,
    /// duration, authored design speed (the rate divisor), variation frequency, replay range —
    /// the variation/replay instrument (sequences sharing an id are its
    /// variation chain).
    M2seq {
        /// Internal path to the `.m2` (forward or back slashes accepted).
        internal_path: String,
    },
    /// Dump an M2's **camera table** by raw file index — the index space a `<Model>` widget's
    /// `Model:SetCamera(n)` walks: type, rest eye/target, diagonal fov, near/far,
    /// roll, and each track's key count (which says still rig vs authored path). The
    /// `cameraLookup` table is printed beside it — the portrait bake selects through that, the
    /// pane does not.
    M2cam {
        /// Internal path to the `.m2` (forward or back slashes accepted).
        internal_path: String,
    },
    /// Dump an M2's animation EVENT keyframes per sequence (`$CSS`/`$CAH`/`$AH0-3`/`$CPP`/`$HIT`…,
    /// time + payload) — the event-order instrument (whether `$CPP` precedes the
    /// impact tag decides defense-anim vs flinch on the shared swing record).
    M2events {
        /// Internal path to the `.m2` (forward or back slashes accepted).
        internal_path: String,
    },
    /// Dump an M2's attachment points (id + bone) — which sheath/held/effect anchors a model
    /// actually has (a placement whose id is absent hangs nothing).
    M2attach {
        /// Internal path to the `.m2` (forward or back slashes accepted).
        internal_path: String,
    },
    /// Dump an M2's animation-channel summary: sequence-0 bone motion, global-sequence bone
    /// channels, transparency/color track key counts, texture transforms, and — per particle
    /// emitter — its full def: shape, blend mode, head/tail, emission-rate keys (burst emitters
    /// key `0 → peak → 0`), lifespan/speed/scale, position/area, resolved texture, and the
    /// over-life color/alpha/size ramps. The one-command diagnosis for "this effect looks wrong /
    /// doesn't show"; paired with `doodadscan`.
    M2anim {
        /// Internal path to the `.m2` (forward or back slashes accepted).
        internal_path: String,
    },
    /// Dump an M2's bone table: KeyBoneID, flags (billboard bits tagged), the `flags & 0x7`
    /// parent-ignore bits spelled out (`ign` — which of the parent's Translate/Scale/Rotate the
    /// bone refuses), parent, pivot, and which sequences key each bone (T/R/S key counts) — the
    /// bone-attach / billboard-geometry instrument (which bone a rider
    /// actually rides and whether it faces the camera; 0945: what it actually inherits from its
    /// parent, which is how a saddle travels with the gallop without rotating with it).
    M2bones {
        /// Internal path to the `.m2` (forward or back slashes accepted).
        internal_path: String,
    },
    /// Dump an M2's render batches as the renderer sees them: geoset, blend/flags
    /// (emissive/additive/two-sided/billboard/depth), texture, vertex count, and whether an
    /// alpha/UV animation baked — the "why doesn't this part of the model draw (or draw wrong)"
    /// instrument (the Orgrimmar-bonfire missing-flame diagnosis).
    M2batch {
        /// Internal path to the `.m2` (forward or back slashes accepted).
        internal_path: String,
    },
    /// Dump an M2's **per-sequence material alpha**: every colour-alpha/transparency track's raw
    /// keys, then the combined per-batch factor for EVERY sequence band — the "which batches does
    /// the reference hide, and in which animation" instrument. A `HIDE` cell is a batch the real
    /// client skips outright in that sequence (`A <= 0` culls before the blend mode is read,
    /// `0x707b3a`–`0x707b5c`). Pair with `m2batch` (the batch -> track wiring) and `m2seq` (the
    /// bands).
    M2alpha {
        /// Internal path to the `.m2` (forward or back slashes accepted).
        internal_path: String,
    },
    /// Dump ONE M2's particle emitters in full — every authored field of every record, as the
    /// runtime parses it: bone/position/shape/blend/head-tail, texture + atlas, the decoded file
    /// flags, the rate/gate timing and the nine emission tracks (flagged `ANIM` with their keys
    /// when they actually move), drag/spin/tail/twinkle, spline and geometry/recursion models,
    /// and the over-life colour/size/flipbook ramps. Closes with a derived read per emitter —
    /// steady-state live count, ballistic reach, size range — so "this flame is a bright blob,
    /// not a spread-out fire" is answered from the asset before anything is blamed on the
    /// renderer. The per-model counterpart to the corpus sweeps (`partcensus`, `partscan`).
    M2part {
        /// Internal path to the `.m2` (forward or back slashes accepted).
        internal_path: String,
    },
    /// Sweep every `.m2` and report where **addressing an attachment by id** through the model's
    /// AttachLookup (the reference's `0x710310`) disagrees with scanning its record table — the
    /// duplicate-id census behind decision 0805 (item glows hang on ids 0..4 of the item model).
    Attachscan {
        /// Optional path prefix to restrict the sweep (e.g. `Item\ObjectComponents\Weapon`).
        prefix: Option<String>,
    },
    /// Sweep every `.m2` in the chain and list the models carrying RIBBON emitters (header
    /// `0x134`) — the population instrument for the ribbon subsystem (weapon trails, streamers):
    /// which content actually authors one, before we build for it.
    Ribbonscan,
    /// Sweep every `.m2` and census its particle emitters' over-life **flipbook** fields: cell
    /// ramps that play BACKWARDS (`begin > end` — legal, shipped, and fatal to a reader that
    /// clamps into the pair), tail streaks whose ramp differs from the head's, indices past the
    /// atlas (the reference wraps the column and lets the row run off), per-segment repeat counts,
    /// and the two degenerate shapes the reference itself falls back on.
    Cellscan,
    /// Sweep every `.m2` (optionally under a path prefix) and list the models whose MATERIAL
    /// table authors the MULTIPLY blend modes 5 (Mod) / 6 (Mod2x) — the ARMORREFLECT sheen
    /// family: the population instrument for the multiply-blend mechanism.
    Blendscan {
        /// Internal-path prefix filter (e.g. `item\objectcomponents\weapon`), case-insensitive;
        /// all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` (optionally under a path prefix) and classify its BILLBOARD usage:
    /// which arms it authors (spherical / lock-X / lock-Y / lock-Z), and whether geometry rides
    /// them DIRECTLY (verts skinned to the billboard bone — the per-batch card path) or
    /// INHERITED (verts on a descendant — the joint-palette path). The
    /// population instrument for the billboard mechanism: which arms real content exercises, so
    /// a class of spell visuals is closed by mechanism instead of tested spell-by-spell.
    Bbscan {
        /// Internal-path prefix filter (e.g. `spells`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` (optionally under a path prefix) and classify each BILLBOARD batch by
    /// which way its geometry FACES. A billboard bone puts the model's +X toward the viewer, so a
    /// batch's winding normal decides whether it is ever seen: +X faces the camera, −X faces away
    /// and — single-sided — is backface-culled by the reference from every angle. The population
    /// instrument for "the card renders but shouldn't": the away+single-sided list is exactly the
    /// authored placeholder geometry the reference hides and a forced-two-sided renderer reveals.
    Bbfacescan {
        /// Internal-path prefix filter (e.g. `world\generic`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` (optionally under a path prefix) and report models that author flat
    /// ground-plane render geometry: batches whose vertices all sit at model-space z≈0 (WoW axes,
    /// Z up), which sloped terrain buries (Battle Shout's crescents are the canonical case — 6
    /// batches, each a 4-vert quad, every vertex exactly z=0, each quad skinned 100% to a single
    /// bone). Flat batches sub-classify QUAD-1BONE (that exact crescent shape) vs OTHER-FLAT — the
    /// population instrument for how general the ground-plane renderer mechanism must be.
    Groundscan {
        /// Internal-path prefix filter (e.g. `spells`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` (optionally under a path prefix) and census the batches carrying
    /// **degenerate authored vertex normals** — `(0,0,0)`, which the shipped corpus really does
    /// author (the AQ40 Qiraji Brainwasher's sleeves, Uldaman Ironaya's skirt). The reference
    /// consumes such a normal as the zero vector and its order-2 SH collapses to the flat DC term,
    /// so the surface draws lit; a renderer that `normalize()`s it gets NaN, `clamp(NaN)` floors
    /// the whole lighting factor to 0, and the batch renders PURE BLACK over its correct texture.
    /// The population instrument for that class (bug B134) — `ALL` marks a batch
    /// where every vertex is degenerate, and the tail names the worst-hit models.
    Normalscan {
        /// Internal-path prefix filter (e.g. `creature`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` (optionally under a path prefix) and measure how far its authored header
    /// bounding box — the model's **all-animation** vertex extent, and the box the reference
    /// derives its doodad cull sphere from (`rec+0x5c`/`rec+0x68`) — reaches past its **bind-pose**
    /// vertex extent. The population instrument for decision 1259: an animated placement's submesh
    /// keeps its transform at the placement origin while the joint palette moves its vertices, so a
    /// bind-pose bound culls an object whose geometry is still on screen. The ambient critters are
    /// the extreme — `World\critter\birds\Bird01.m2` authors a 67 yd box around a 1.2 yd body,
    /// because its root bone flies it 64 yd along a circuit. Also reports the reverse (`SHORT`:
    /// bind-pose geometry OUTSIDE the authored box), which is why the fix unions the two boxes.
    Animboundscan {
        /// Internal-path prefix filter (e.g. `world\critter`), case-insensitive; all if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` (optionally under a path prefix) and census the geometry a
    /// **non-character** spawn draws that the reference may not: MULTI-GEOSET models (more than
    /// one `skinSectionId` — only the character compositor selects among them, so every other
    /// spawn path draws all of them), UNTEX batches (no embedded texture and no character runtime
    /// slot) and TINY batches (at most 2 faces). The population instrument behind the stray
    /// untextured-primitive reports; `m2batch` then explains a single model in full.
    Geosetscan {
        /// Internal-path prefix filter (e.g. `creature`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every model named by **GameObjectDisplayInfo.dbc** and resolve what the reference's
    /// GameObject animation arm plays in each reachable `GAMEOBJECT_STATE` × `GAMEOBJECT_ANIMPROGRESS`
    /// substate (the substate table `0x5f3c30`, the `0x8607e4` LUT, the four-way missing-sequence
    /// remap `0x5f3972`) — beside the generic loader seed the same model gets at build (`0x710153`,
    /// animation id 0 through `playableAnimationLookup`). The population instrument for "which
    /// GameObjects can the wire state actually be SEEN on": a model whose every substate lands on the
    /// seed's own sequence is state-blind, so skipping the arm on its GO type costs nothing, while a
    /// STATE-SENSITIVE model renders in the wrong pose the moment its type is left off the machine.
    /// Also counts the models whose pose depends on the missing-sequence remap (benilla plays
    /// nothing there today, i.e. bind pose) and the ones reaching a rate-0 freeze leg.
    Goanimscan,
    /// Sweep every `.m2` and census the **event table's positional half**: the `bone` and
    /// `position` every `M2Event` record carries beside its 4CC. The reference's event dispatchers
    /// hand their arms the event's own world point (the authored `position` through its bone's live
    /// matrix and the model's placement), while a consumer that plays at the model root uses the
    /// placement alone — so this reports, per 4CC, how many records sit off the origin and how many
    /// ride a bone any sequence keys. Where both are zero the two are the same point.
    Eventmarkerscan {
        /// Internal-path prefix filter (e.g. `creature`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Census the **GameObject display sound slots** (`GameObjectDisplayInfo.Sound[0..9]`) against
    /// the only thing that can reach them. Exactly one function in the reference reads those
    /// columns (`0x5f4010`) and it is called only from the GO M2 anim-event dispatcher
    /// (`0x5f3e20`): `$GO0..5` -> slots 0..5, `$GC0..3` -> slots 6..9. So a filled column is
    /// audible only when the display's own model authors the matching event tag AND that tag sits
    /// on a sequence the GameObject animation arm can actually play. Reports, per slot: columns
    /// filled, of those how many are tagged, how many of those are on an armable sequence, and how
    /// many name a LOOPING (0x200) kit — the flag that selects `0x5f4010`'s emitter-pool lane over
    /// its one-shot lane.
    Goslotscan,
    /// Sweep every `.m2` (optionally under a path prefix) and census the models whose batch
    /// visibility is PER SEQUENCE — geometry the reference draws in one animation and skips in
    /// another (the verified `A <= 0` alpha cull). The population instrument for "a single-sequence
    /// material bake draws the wrong set"; `m2alpha` then explains one model in full.
    Alphascan {
        /// Internal-path prefix filter (e.g. `creature`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` (optionally under a path prefix) and census the **effect-model animation
    /// lifecycle**: which models author the `Stand`(0) -> `Hold`(158) -> `Decay`(159) triple, and
    /// what a consumer that arms ONE sequence and never advances renders for each. A spell-effect
    /// model puts its grow-in in the clamping `Stand`, its sustained pulse in the looping `Hold`
    /// and its fade-out in `Decay` — so arming only the first freezes on the birth's last frame for
    /// the effect's whole life (the frozen Ice Barrier shield). The population instrument for that
    /// class: FREEZE / hold-loops / decay-only counts, corpus-wide; `m2seq` explains one model.
    Fxlifescan {
        /// Internal-path prefix filter (e.g. `spells`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` (optionally under a path prefix) and measure how far our SKELETAL parse
    /// sits from the reference's sampler, per bone track per sequence band: empty bands (our
    /// nearest-key clamp vs the file's own `interpolation_ranges` window), held band edges (our
    /// hold vs the reference's ongoing lerp toward the next key), and STEP tracks (`interp == 0`,
    /// which the reference copies and we interpolate). The population instrument behind decision
    /// 0133's named residual — and the safety check that a band's keys never fall outside the
    /// window the reference searches.
    Bonescan {
        /// Internal-path prefix filter (e.g. `creature`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` (optionally under a path prefix) and census every particle-emitter
    /// FEATURE the corpus authors — shapes, head/tail, blend modes (incl. the folded Mod/Mod2x),
    /// every file-flag bit, spin signs, twinkle gates, atlas tiling, spline emitters, and the
    /// record fields the renderer doesn't parse yet (geometry-model "model particles",
    /// recursion-model child emitters, the rate track's interp word) — each with counts, example
    /// models, and the spells whose visual chain plays them. The population instrument that turns
    /// "this spell looks wrong" reports into a ranked mechanism worklist, closing feature classes
    /// corpus-wide instead of spell-by-spell.
    Partcensus {
        /// Internal-path prefix filter (e.g. `spells`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` (optionally under a path prefix) and list the particle emitters whose
    /// **file slot 0 is dead while a later slot is alive** (`peak0 <= 0 < peakN`). The reference
    /// samples the PLAYING sequence's rate window every frame, so an emitter whose burst is keyed
    /// in a later variation is ordinary content — but a consumer that PINS slot 0 renders it as
    /// nothing at all, for ever, on every placement, while the emitter still builds, pools and
    /// ticks. The population instrument for that silent class (found on
    /// `BlastedLandsLightningbolt01.m2`, the Blasted Lands strike that never fires).
    Partslotscan {
        /// Internal-path prefix filter (e.g. `world`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` (optionally under a path prefix) and census the batches the bake routes
    /// to a **per-placement material** — the ones whose UV loop or M2Color RGB tint loop is not the
    /// same in every FILE sequence slot, so no material shared by every instance can be right for
    /// them. The texture-transform twin of `partslotscan`.
    ///
    /// It asks the **bake**, not a copy of it: `RenderSubmesh::uv_seq` / `rgb_seq` are `Some`
    /// exactly when `SeqLoops::uniform()` refused the shared lane, so the census and the runtime
    /// cannot disagree by construction. It used to transcribe the sampler instead — it was written
    /// to SIZE this fix, before the fix existed — and once the fix landed the twin drifted from it:
    /// the transcription compared periods and value EXTENTS at 1e-3, which calls
    /// `Spells\AdrenalineRush_Cast_Base` the same loop in both slots, where the bake's own tail key
    /// differs by a full 1.0 (slot 0 ends blue, slot 1 returns to red).
    ///
    /// Per (batch, channel): **SHARED** — the set is `None`, the slots agree, nothing changes for
    /// it (with the sub-count that actually animates on that lane) — and **PER-PLACEMENT**, the set
    /// is `Some`. The latter splits by WHY the slots disagree, which is the question of whether
    /// `uniform()`'s exact float equality is *earning* the routing it does: **DEAD-0** (slot 0
    /// bakes nothing while a later slot animates — the B98 shape, `BlackrockStatueLavaBubble.m2`
    /// keying its whole UV flipbook inside file slot 1, a 50/50 variation of animation id 0, so the
    /// slot-0 bake returns `None` and every placement is frozen for ever), **WRAP-ONLY** and
    /// **KEYS-EPSILON** — the over-application buckets, where the slots hold the same authored loop
    /// and are split only by the wrap flag or by sub-epsilon noise in a rebased key — and
    /// **REAL-DIFFERS** (genuinely different loops, or slot 0 alive against a dead later slot).
    /// Over the shipped corpus both over-application buckets are **empty**, so nothing is on the
    /// per-placement lane that a coarser comparison would have left off it.
    ///
    /// Each bucket closes with its content family and a named worst-hit tail (batches, file
    /// sequence slots). `World\` is the only family the per-placement lane reaches at all — a
    /// placed doodad or WMO prop; `Creature\`/`Spells\` and the `World\Goober\` GameObject
    /// displays resolve their own sequence through the entity lane — so a bucket living outside it
    /// costs nothing whatever it says.
    Uvslotscan {
        /// Internal-path prefix filter (e.g. `world`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` (optionally under a path prefix) and list the models whose animation is
    /// authored **entirely outside the bone tracks**: sequences exist, per-sequence consumers exist
    /// (particle emitters, ribbons, material alpha/colour, UV transforms), and not one sequence
    /// keys a bone. A sequence clock that rides a bone-animation clip has nothing to ride on such a
    /// model — no clip, no player, no "which sequence, how far in" — so every per-sequence consumer
    /// degrades to file slot 0 at t = 0, for ever. The population instrument for that silent class
    /// (found on the Molten Core rune + flame ring). `[GO]` marks a
    /// `GameObjectDisplayInfo` model — the hosted-clock lane, where the freeze is total.
    Seqclockscan {
        /// Internal-path prefix filter (e.g. `world`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` (optionally under a path prefix) and list batches whose texture is authored
    /// **CLAMP** (`M2Texture.flags` bit 0/1 clear) while the batch's UVs run **outside `0..1`** — the
    /// population a repeat-sampling renderer draws wrong. That margin is deliberate: clamped it
    /// samples the sheet's transparent border and the card fades to nothing; wrapped it folds into
    /// the opposite edge and draws as solid geometry with a seam at the crossing (
    /// bugs B52/B96 — the Dun Morogh snow-firs and the Plaguelands bush).
    Uvwrapscan {
        /// Internal-path prefix filter (e.g. `world`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every model the **spell-visual chain** can reach — every `SpellVisualEffectName`
    /// path a kit's ten effect slots, a `SpellVisual` row's missile model or its dest-anchored
    /// model names — and census the batches whose **texture transform animates**, then classify
    /// each by what a consumer that runs NONE of it renders.
    ///
    /// Decision 0271 deferred this channel on the claim that "no effect model in the current
    /// corpus needs it"; this is that claim, made countable — it is what 2282 read to size the
    /// missing scroll, and it is how the same question was re-asked of the unit / GameObject /
    /// held-item corpus that 2295 then fixed (`entityuvscan`, its twin). It asks the **bake**,
    /// not a transcription of it: a batch is in scope
    /// exactly when `tex_anim` emitted a loop on any of the three channels, the same test the
    /// lanes that DO run them use (`ui_models` 2019, `spell_fx` 2282).
    ///
    /// The classes are what a frozen batch draws, judged from the texture's own alpha through its
    /// authored address mode — because a CLAMP-authored sheet's border is what a UV outside `0..1`
    /// samples, and on this corpus that border is transparent:
    ///
    /// - **INVISIBLE** — frozen, every texel the batch reaches is transparent, while the scroll
    ///   reaches painted ones: the batch renders **nothing at all**. `Spells\SwipeCaster.m2`
    ///   (druid Swipe) is the class: two 51-vertex claw-trail strips whose UVs are authored at
    ///   `u[+0.945..+1.944]` over a 16×16 CLAMP sheet, so frozen they sample column 15 alone —
    ///   alpha 0 — and the whole of their visible existence is the `−0.97` U scroll.
    /// - **FROZEN** — it draws, statically: the scroll is the motion it loses.
    /// - **HELD** — keyed to a constant non-identity offset, so a lane that seeds none draws it
    ///   mis-registered rather than still.
    /// - **NEVER** / **UNKNOWN** — flagged, never counted as INVISIBLE: nothing painted at any
    ///   point of the loop, or no alpha lane to judge from (`Mod`/`Mod2x`, an undecodable sheet).
    ///
    /// Each batch prints its per-axis reasoning (address mode, authored and frozen UV spans, the
    /// texel indices those reach) so the call is checkable, and the report closes with the INVISIBLE
    /// listing joined back through the chain: which spells reach it, and through which lifecycle
    /// stage (`precast`/`cast`/`impact`/`state`/`channel`/`missile`/`area`).
    Fxuvscan {
        /// Internal-path prefix filter (e.g. `spells`), case-insensitive; all reachable effect
        /// models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every model the **ENTITY lane** can render — every `CreatureDisplayInfo` →
    /// `CreatureModelData` body (NPCs, critters, mounts, and every PLAYER, which resolves through
    /// the same chain), every `GameObjectDisplayInfo` model, every
    /// `ItemDisplayInfo` left/right model joined to the `Item\ObjectComponents\` folder the
    /// archives actually hold it in, and the corpse lane's `<Race><Sex>DeathSkeleton` bone piles —
    /// and census the batches whose **texture transform animates**, then classify each by what a
    /// consumer that runs NONE of it renders.
    ///
    /// The twin of `Fxuvscan`, asked of the corpus that record's fix did NOT reach. Decision 2282
    /// gave the spell-effect lane its texture transform and named this as its first deferral:
    /// `model_render::batch::Materials::entity_variants` passed `play_uv = false`, and `build`
    /// does `play_uv.then_some(sub.uv_anim.as_ref()).flatten()` — so every unit, player,
    /// GameObject and held-item batch was handed no loop and drew its authored UVs, untransformed,
    /// for ever. Nobody had measured how much content that is; this is that measurement, it is
    /// what sized **decision 2295**, which closed it, and it shares `fxuvscan`'s whole per-batch
    /// reader (`uv_batch`) so the two corpora cannot be judged by two different rules. It stays
    /// because the census is how the claim "this lane needs no channel" is kept checkable — which
    /// is the one thing 0271's deferral was missing.
    ///
    /// **Read the corpus from the tables, never from a path prefix.** "The path looks broken" and
    /// "the path is reachable" are different questions and only the second one is worth a session
    /// (2282's own closing note). A held item's directory is not even a column — the equipment lane
    /// picks `Weapon`/`Shield`/`Shoulder`/`Head`/`Ammo`/`Quiver` from the slot the item is worn in
    /// — so the sweep asks the archives which folder holds each display's basename, and expands a
    /// helm stem into its sixteen per-race/sex files the way the attach does.
    ///
    /// The classes are `fxuvscan`'s, judged the same way — from the texture's own alpha through the
    /// batch's authored address mode (INVISIBLE / NEVER / FROZEN / HELD / UNKNOWN; that command's
    /// doc is where they are explained) — with one thing this corpus adds: a CREATURE batch's
    /// `Monster1/2/3` sheet is **blank in the M2** and filled per display from
    /// `CreatureDisplayInfo.textureVariation`, so one batch has as many sheets as the model has
    /// skins and the verdict is asked once per skin. A character composite (body atlas, hair,
    /// object skin) has no authored sheet at all and reads UNKNOWN, which is the honest answer
    /// rather than a silent pass.
    ///
    /// **The load-bearing column is the CLOCK**, because it decides the SHAPE of the fix and not
    /// merely its size:
    ///
    /// - **GSEQ** — every live loop rides a global sequence, a free-running per-scene clock the
    ///   reference anchors once per instance at attach. One shared material uniform is faithful
    ///   there; that is 0136 choice 1's lane, already built.
    /// - **BAND** — every live loop rides its sequence band, i.e. the instance's own play head. Two
    ///   units playing different animations, or the same one at different phases, are at different
    ///   offsets, and no shared uniform can serve both: that is 2282's per-instance `UvLoop`.
    /// - **MIXED** / **HOLD** — the batch's channels disagree, or it is keyed to a constant
    ///   non-identity offset that needs a seed and no clock at all.
    ///
    /// Each affected batch prints its three channels (translation / rotation / scaling — a sweep
    /// that looked only at translation would have missed `GroundingTotem_Impact` entirely, which is
    /// scale-only), its per-axis texel reasoning, both frozen verdicts, its clock, which file
    /// sequence slots carry the keys and what those sequences are *called*, and — where the bake
    /// refused the shared lane — `uvslotscan`'s own `uniform()` verdict for the same set. The
    /// report closes with every affected model sorted by POPULATION: how many table rows can put it
    /// on screen, which is the difference between one gnome terminal and every murloc in the game.
    ///
    /// **And then the same corpus again for the M2COLOR TINT**, because the UV channel is not the
    /// only one this lane drops. `build` passes `sub.rgb_anim.as_ref()` *unconditionally* — there
    /// is no `play_rgb` to flip — so a tint-animating entity batch is seeded at the loop's first
    /// key and then never re-sampled, because `doodad_anim::register_tint`'s only call sites (like
    /// `register_uv`'s) are in the world streamer's `assemble.rs`. The hole is the missing
    /// REGISTRATION, not a missing argument, which is why flipping `play_uv` alone would only
    /// trade one frozen frame for another.
    ///
    /// A tint is a **multiply**, so its failure mode is the wrong colour rather than missing
    /// geometry, and the classes say how wrong: BLACK (the frozen tint kills the batch), STRONG,
    /// SLIGHT, NEGLIGIBLE — bands over one printed number, the worst per-channel distance between
    /// the frozen value and anything the loop reaches. Two qualifications the raw count would
    /// overstate without: a batch whose slot 0 bakes nothing seeds **white**, so it is the right
    /// colour until a later slot's animation plays and wrong only *during* it (the `slots` line
    /// names which); and the **ALPHA** channel is counted beside them precisely because this lane
    /// DOES serve it — `attach::dress::spawn_part` gives every part a `MatAnim` that
    /// `sample_mat_anim` ticks per instance. One of the three material-animation channels runs.
    Entityuvscan {
        /// Internal-path prefix filter (e.g. `creature`), case-insensitive; all reachable entity
        /// models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` (optionally under a path prefix) and census the batches whose texture
    /// coordinates are **GENERATED, not authored** — the sphere-map environment stages
    /// (`texture_unit_lookup[texCoordSet] > 2`, the reference's gate at `0x70b8bd`). Such a batch
    /// carries no usable UVs *by design* (the artist collapses the mesh onto one point because the
    /// runtime supplies the coordinates), so a renderer that reads the vertex UV paints the whole
    /// surface in **one texel** of a reflection sheet — silently, with no missing geometry to
    /// notice. `DEGENERATE` marks exactly that population; the rest carry unused leftover UVs and
    /// misdraw as a static smear instead. The instrument behind the Deeprun Tram glass tube.
    Envmapscan {
        /// Internal-path prefix filter (e.g. `world`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` and report which sampler ADDRESS MODES the corpus asks of each texture
    /// path, and how many paths are asked for more than one — the design check behind decision
    /// 0763 (the mode lives on the sampler, which rides the `Image`, which is keyed by path).
    Texmodescan {
        /// Internal-path prefix filter, case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` (optionally under a path prefix) and count the two halves of the
    /// **owner-last draw-order** law per model: the EFFECTS it authors (particle emitters +
    /// ribbon trails) and the TRANSPARENT-pass batches of its own body those effects must draw
    /// after, plus the model's reach and the draw-order rung that reach
    /// produces. The population instrument for "how much does this fix, besides the one creature
    /// it was found on": a model with effects AND transparent batches of its own is one whose
    /// effects our distance sort could interleave with its own body; one without never had the
    /// defect at all.
    Fxordercensus {
        /// Internal-path prefix filter (e.g. `creature`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` (optionally under a path prefix) and census the **3-D model particles**:
    /// emitters carrying a geometry-model reference, whose shard instances render the GEOMETRY
    /// model's own submeshes stamped with the OWNER model's owner-last draw-order rung. Per
    /// (owner, geometry) pair: the owner's rung and reach plus the geometry's material family
    /// tuples (blend, two_sided, additive, no_depth_write, no_depth_test) — the exact keys the
    /// shard materials are built from. The ground truth sizing the pipeline-warm menagerie's
    /// shard rows: rung histogram, family-tuple set split by transparent-pass membership, and
    /// the unresolvable-path bound.
    Shardcensus {
        /// Internal-path prefix filter (e.g. `spells`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` (optionally under a path prefix) and list the models where the
    /// **loader-idle sequence is not file slot 0** while benilla's render content gate leaves it
    /// unarmed — so every per-sequence bake (emission rate/gate, the nine parameter channels,
    /// material alpha) degrades to slot 0, a sequence the instance is not playing. On the
    /// `DuelingFlag`-shaped GameObjects (Spawn/Stand/Despawn) slot 0 is the **Spawn flourish**:
    /// the population instrument for decision 0936, found on the Stormwind battlefield banner
    /// pouring its spawn sparkle-tails for ever.
    Idleslotscan {
        /// Internal-path prefix filter (e.g. `world`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` and census the **animation-driven sound emitters**: the models whose
    /// sequences carry a `$DSL` (doodad sound loop) / `$DSE` (its release token) / `$DSO` (doodad
    /// sound one-shot) / `$SND` (generic one-shot) marker, the `SoundEntries` kit each names with its 3D parameters, and —
    /// the column this exists for — whether the carrying sequence is REST-posed, i.e. one the
    /// render content gate never builds a rig for. A placed lamp's hum is a single
    /// `$DSL` on a sequence that keys no bone at all, so the whole class is unreachable through an
    /// `AnimationPlayer`: the population instrument for "every world doodad is silent".
    Soundeventscan {
        /// Internal-path prefix to limit the sweep (e.g. `world`); all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` (optionally under a path prefix) and list the models whose PARTICLE
    /// emitters carry any of the given file-flag bits (`M2ParticleEmitter+0x04`) — the
    /// population instrument for particle-flag mechanisms (which content actually authors
    /// e.g. the `0x1000` plane-quad leg), so a class of effects is closed by mechanism
    /// instead of tested spell-by-spell.
    Partscan {
        /// The flag mask to match, hex or decimal (e.g. `0x1000`).
        mask: String,
        /// Internal-path prefix filter (e.g. `spells`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` (optionally under a path prefix) and report which models author M2
    /// dynamic LIGHT blocks — the population instrument for the mechanism (
    /// `0x718960`). Per model: its point (`type==1`, the GL
    /// hot-spot caster) vs directional (ambient-feed) light counts, then per point light its
    /// bone/position/diffuse colour×intensity/attenuation/visibility-gate. The closing summary —
    /// totals, a breakdown by top-level content family (Creature/Item.ObjectComponents/
    /// Character/World/Spells/other), and a diffuse-palette tally — is the real deliverable:
    /// benilla only spawns these lights for ADT-placed doodads and WMO props today, so it answers
    /// how much of the entity path (creatures, held items, GameObjects) is missing them.
    M2lightscan {
        /// Internal-path prefix filter (e.g. `item\objectcomponents`), case-insensitive; all
        /// models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every WMO ROOT and cross-tab the two halves of the skybox mechanism: the root's
    /// **MOSB** skybox model against its groups' `0x40000` flag. `0x40000` is undocumented, so this
    /// is what *identifies* it — across all 815 roots the bit never appears without a MOSB. Note
    /// what that does and does NOT buy: it establishes `flag => MOSB`, never which group the
    /// renderer tests (the reference's law is the flood-VISITED group, `0x6b42e0`; decision 0773
    /// correcting 0767).
    /// Also the population instrument: which 1.12 buildings replace the `Light.dbc` gradient dome
    /// with an authored sky (Stratholme's burning city is the only one reachable), and how much
    Skyboxscan,
    /// Dump all 18 `LightIntBand` rows of the `Light.dbc` entry covering a world position at a
    /// time of day — the band-semantics instrument (which row holds which colour at which hour;
    /// the celestial-diffuse band question). ⚠ This is ONE params record RAW —
    /// near a sphere's falloff edge the LIVE light is mostly the continent global and looks
    /// nothing like these rows; `lightblend` shows what actually wins at the position.
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
    /// Dump every `LightIntBand`/`LightFloatBand` row of an explicitly named `LightParams` id — the
    /// instrument for rows **no position can reach**, because nothing in `Light.dbc` references them:
    /// magma submersion reads the fixed global row 7 and slime row 6 (VERIFIED `0x6d2371`), not the
    /// zone's underwater slot.
    Lightparam {
        /// `LightParams.dbc` id (1-based).
        id: u32,
        /// Game minute-of-day (0..1440; 720 = noon).
        minute: u32,
    },
    /// Dump the **per-weather-slot** light resolve at a world position: which `LightParams` id each
    /// of the five `Light.dbc` slots names (clear / clear-underwater / storm / storm-underwater /
    /// death), which of them are UNSET (and so fall back to clear), and the fog/ambient/diffuse the
    /// blend resolves for each. `lightbands` only ever shows the clear slot, so an underwater or
    /// ghost atmosphere that reads wrong could not be told from data that simply has no such record.
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
    /// Dump every light SPHERE covering a world position — distance, falloff, blend alpha, each
    /// sphere's clear ambient/sun — then the faithful area-blend result vs the single `pick_light`
    /// sample. The instrument for "which palette actually wins HERE": a position near a zone
    /// sphere's falloff edge resolves mostly the continent global, and the raw per-params band dump
    /// (`lightbands`) cannot show that.
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
    /// Bulk-scan placed doodads (MDDF) and WMO doodad-set-0 props (MODF → MODS/MODD) across a
    /// `(2·tile_radius+1)²` block of ADT tiles around a world position, and report how much of that
    /// content animates (see `m2anim`) — the doodad-animation perf/scope instrument.
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
    /// Dump a WMO root's placed-prop tables: every MODD doodad with its MODS set membership and
    /// its OWNING group(s) read from the group files' MODR lists — the relation the reference
    /// instantiates from (a *visible* group's own refs, `0x695aa0`), so a prop referenced by NO
    /// group is one the real client never creates at all (the divergence decision 0689 names,
    /// which benilla still spawns). The "which props exist in this building, who owns them, and
    /// which would the reference even draw" instrument.
    Wmodoodads {
        /// Internal path to the WMO **root** (forward or back slashes accepted).
        internal_path: String,
        /// Case-insensitive substring of the prop's model path (e.g. `lightray`); all if omitted.
        filter: Option<String>,
    },
    /// Sweep every WMO **root** (optionally under a path prefix) and list the placed MODD props
    /// the INTERIOR lighting lane commits as **literal black**. That lane's entire base light is
    /// the MODD entry's own baked colour (ambient `cap96`, diffuse `floor112` — `0x694e90` →
    /// `0x6a77e0`), and the floor leg is a hue-preserving scale by `112/max`, so a colour of
    /// exactly `#000000` has nothing to raise and both words come out zero: the prop is lit by
    /// nothing but its owning group's MOLR fixtures, and a group with none in range draws a pure
    /// black silhouette. The `ALSO-EXT` column is the divergence half — a prop an EXTERIOR group's
    /// MODR *also* names is one the reference reaches through that group too, while our
    /// single-instance first-referrer-wins ownership pins it to the interior lane from every angle.
    Darkpropscan {
        /// Internal-path prefix to limit the sweep (e.g. `world\wmo\azeroth`); all if omitted.
        prefix: Option<String>,
    },
    /// List individual doodad (MDDF) and WMO (MODF) placements around a world position whose
    /// model path contains a substring — position, Euler rotation (deg), scale, uniqueId. The
    /// "point at a thing in the world, tell me exactly how it's placed" instrument: an
    /// orientation bug needs the placement rotation as ground truth, which `doodadscan`'s
    /// aggregates never show.
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
    /// Print the terrain MCSH baked-shadow bit at a world position — the per-instance
    /// terrain-shade selector an exterior M2 doodad samples once at its base (sun intensity
    /// 2.5 lit / 0.5 shadowed, `0x69e4ad`) — plus an ASCII texel
    /// neighborhood. A doodad base near a shadow edge sits one ~0.52 yd texel from the OTHER
    /// intensity family, so two adjacent fence pieces can land on opposite sides — the
    /// "why is ONE of these blown out" instrument.
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

/// The grouping/lookup key for a model path referenced from MDDF/MODD (`doodadscan`'s instance
/// tables): lowercase, `.mdx`/`.mdl` → `.m2`. Mirrors `benilla_formats`' own (crate-private)
/// `model_path` normalization so two placements of the same model authored under different
/// casing/extension collapse to one row instead of double-counting it.
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

/// `"yes"`/`"-"` for a compact table cell.
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
            // Note which archive the file resolved from (handy for debugging overrides).
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
