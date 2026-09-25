//! **The world API wall** — the doorway of `benilla-world`, counted from source on every
//! `cargo test`, so it can never be measured once and then quietly widen.
//!
//! Decision 1160 splits the world renderer out of `benilla-app` into `benilla-world`, and its
//! *secondary* falsifier is a number: if the engine's public vocabulary is much larger than a
//! designed API, the cut line is wrong. 1163 measured it — **214 distinct engine items named by
//! gameplay code** — and set the gate at **forty**, with the sort into DOWN / PUBLISH / CLOSE as
//! the work. 1164 is that sort.
//!
//! A number in a decision record is a number nobody re-measures. This is the same measurement as
//! a test, so every commit that closes a leak shows up as [`CEILING`] going down, and every commit
//! that opens a new one fails the gate the day it lands rather than three weeks later.
//!
//! **It has two lives, and it switches by itself.** Before the move, the boundary is a naming
//! convention: gameplay files reaching `crate::<engine module>::…`. After the move it is a real
//! crate, and the same count is `benilla_world::…` in `benilla-app`'s sources. The scan below
//! looks for `crates/benilla-world` and measures whichever world exists — so the ratchet survives
//! stage two instead of being deleted by it.
//!
//! What it deliberately does **not** count: `#[cfg(test)]` bodies (a test naming an internal is
//! not an API consumer), doc comments (a `[`crate::x::Y`]` link is prose), and the composition
//! root's plugin registrations are counted like anything else — 1164's plugin-group collapse has
//! to actually happen for them to stop counting.
//!
//! **Since decision 2338 the number that gates is the LEAKS, not the surface.** For five weeks
//! the one number went up twenty-seven times, every raise a paragraph saying "a PUBLISH", and
//! never once down: the falsifier 1163 set at forty had fired, and the ratchet had become a
//! diary. So the sort is data now. [`PUBLISHED`] is the designed API, each row with the record
//! that published it; [`SORTED_LEAKS`] is what 1164 filed to close and how; anything else that
//! crosses is an unsorted leak. The surface and the published count are reported; only the leak
//! count is ratcheted, and it ratchets toward zero. Adding a `PUBLISHED` row is the act that
//! raising the ceiling used to be — a claim made in review, with its record — and a row nothing
//! names any more fails the test until it is deleted, so the tables cannot rot.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The engine's module set inside `benilla-app` — the cut line of 1160, written down.
///
/// This list is the pre-move definition of "the world". It is load-bearing in exactly one way: an
/// engine module missing from it makes its leaks invisible to this test. It goes away at stage
/// two, when `crates/benilla-world/` becomes the definition and the scan switches to the crate
/// path (see the module docs).
const ENGINE_ROOTS: &[&str] = &[
    "art_scope",
    "assets",
    "billboard",
    "clouds",
    "clutter",
    "collision",
    "debug_panel",
    "decal",
    "dev_state",
    "doodad_anim",
    "entity_shade",
    "exterior_cull",
    "ffx_glow",
    "instance_tint",
    "interact",
    "interior",
    "lighting",
    "liquid",
    "map_proj",
    "mesh_tag",
    "model_fade",
    "model_forms",
    "model_render",
    "modkeys",
    "particles",
    "perf",
    "pipe_warm",
    "ribbons",
    "rig_anim",
    "rig_palette",
    "schedule",
    "sky",
    "sky_order",
    "skybox",
    "sun",
    "surface",
    "terrain",
    "terrain_stream",
    "view",
    "water_fx",
    "wdl",
    "weather",
    "wmo_portal",
    "world_census",
    "world_plugins",
    "world_point",
    "world_unit",
    "world_map",
    "zfill",
];

/// The instruments — `debug_panel`, `perf`, `pipe_warm`, `art_scope`. They live inside the engine
/// module set today (the world viewer boots them), but 1160 puts instruments at the *top* of the
/// stack, not the bottom: extracting them early would publish hundreds of internals as permanent
/// API shaped by what a debugger wanted to poke. So they are not part of the doorway being
/// measured, and gameplay reaching into them is a different problem with a different record.
const INSTRUMENT_ROOTS: &[&str] = &["art_scope", "debug_panel", "perf", "pipe_warm"];

/// The instruments that ended up **above** the engine — `benilla-app` modules that consume
/// `benilla-world` like any other caller.
///
/// 1164's rule is that an instrument's surface is not the engine's designed API: "1160 puts
/// instruments at the top of the stack, and their surface is a different record's problem".
/// 1167 corrected the *geography* — the compiler proved these three read the game and cannot live
/// inside the engine — but the rule it was attached to still holds, and for the reason 1163 gave:
/// an API whose target is set by what a debugger wanted to poke is the same failure as an API
/// shaped by whichever `use` statement got written first.
///
/// So they are excluded from the gated number and **counted separately**, because a rule that
/// hides a number is worse than no rule. `art_scope` is not here: it is registered by
/// `WorldPlugins` and lives inside the engine, so it never crosses.
///
/// `crash` is the fourth: the panic hook that writes the crash report. It is
/// `perf::stall`'s sibling — a diagnostic that writes into `Diagnostics/` and nothing gameplay
/// reads — and the one engine item it names, `log_ring::recent`, is a diagnostic feed kept beside
/// the engine's `LogPlugin` because that is where the layer has to be installed. An API shaped
/// by what a crash report wanted to attach is 1163's failure exactly, so it is counted here, not
/// in the doorway.
///
/// `capture` is the fifth, and the oldest ruling of the five: 1164 §Measured
/// already counted `capture/*` among the instruments outside the doorway (its "21 named only by
/// instruments"), but this list never said so, and a probe's reach — `static_gx::StaticGx`,
/// `liquid::FoamPatch`, `ride_frame::ride_matrix`, the water-effect fixtures — was gated as if
/// the game had asked for it. A probe that reads the engine's own state is what a probe is for.
const INSTRUMENT_CONSUMERS: &[&str] = &["capture", "crash", "debug_panel", "perf", "pipe_warm"];

/// Is this file one of the app-side instruments?
fn is_instrument_consumer(rel: &str) -> bool {
    let root = rel.split(['/', '.']).next().unwrap_or("");
    INSTRUMENT_CONSUMERS.contains(&root)
}

/// **The designed API** — every engine item game code may name, and the record that published
/// it. `1164` is the sort's own PUBLISH bucket and its facades; a four-digit
/// number is the record whose paragraph in the raise log below said "a PUBLISH"; `wall` is a
/// raise-log paragraph that published the item without naming a record (grep the name below);
/// `1167` published the facades the crate move landed; `2338` is the boot seam — the process-level
/// entries a binary needs to run the engine at all (`worldview::run`, `tuned_default_plugins`,
/// `BuildId`, the background-window and thread-QoS helpers), the same class as the three plugins
/// the log already published. **Adding a row is a claim made in review, with its record.** A row
/// no game file names any more fails the test until it is deleted.
const PUBLISHED: &[(&str, &str)] = &[
    ("bgwin::BgWinPlugin", "wall"),
    ("bgwin::background_run", "2338"),
    ("bgwin::no_pixel_run", "2338"),
    ("billboard::BillboardCard", "1164"),
    ("boot::tuned_default_plugins", "2338"),
    ("build_id::BuildId", "2338"),
    ("build_id::banner", "1179"),
    ("collision::ColliderEpoch", "1384"),
    ("collision::MoverTraceExclusions", "1767"),
    ("collision::WorldCollision", "1164"),
    ("decal::WorldDecal", "1164"),
    ("dev_state::STILL_INPUTS_CHANGED", "1979"),
    ("doodad_anim::AnimMatPart", "2295"),
    ("doodad_anim::DoodadAnimHost", "1365"),
    ("doodad_anim::MatAnim", "1164"),
    ("doodad_anim::TintLoop", "2295"),
    ("doodad_anim::UvLoops", "2282"),
    ("doodad_anim::register_entity_uv", "2295"),
    ("doodad_anim::register_fx_uv", "2282"),
    ("doodad_anim::register_tint", "2295"),
    ("doodad_anim::spawn_anim_host", "1164"),
    ("ffx_glow::FfxBackdrop", "2234"),
    ("ffx_glow::FfxGlow", "1164"),
    ("ffx_glow::GlueFfx", "1731"),
    ("final_pass::FinalPassTarget", "2206"),
    ("instance_tint::InstanceTintMirrors", "1731"),
    ("instance_tint::InstanceTints", "1164"),
    ("interact::PickMesh", "1164"),
    ("interact::WorldObject", "1164"),
    ("interact::WorldPick", "1164"),
    ("interact::cast_pick_ray", "1164"),
    ("interior::NodeAmbient", "2079"),
    ("lighting::LightBlob", "1164"),
    ("lighting::WorldTime", "1167"),
    ("lighting::WowLighting", "1164"),
    ("mac_quit::MacQuitPlugin", "1528"),
    ("mat_anim_table::MatAnimMirrors", "2023"),
    ("mat_anim_table::MatAnimTable", "1381"),
    ("mat_anim_table::affine_row", "2019"),
    ("mesh_tag::HIGHLIGHT_BIT", "1164"),
    ("model_fade::ModelFade", "1164"),
    ("model_fade::ParentModel", "1164"),
    ("model_fade::RenderFade", "1164"),
    ("model_fade::UnitRenderAlpha", "wall"),
    ("model_forms::ModelForms", "1167"),
    ("model_render::BatchVariants", "1164"),
    ("model_render::EntityUvLane", "2295"),
    ("model_render::M2BatchMaterials", "1164"),
    ("model_render::ModelKind", "1164"),
    ("model_render::ModelPart", "1164"),
    ("model_render::lazy::realize", "1940"),
    ("modkeys::SyntheticHold", "wall"),
    ("particles::ViewThrottled", "1559"),
    ("particles::render::EFFECT_DRAW_STATS", "1955"),
    ("particles::spawn_emitter", "1164"),
    ("ride_frame::RideFrame", "1661"),
    ("rig_anim::AnimParked", "wall"),
    ("rig_anim::GlobalSeqDrive", "wall"),
    ("rig_anim::PosePost", "wall"),
    ("rig_anim::RigAnchor", "wall"),
    ("rig_anim::RigFrame", "wall"),
    ("rig_anim::RigPose", "wall"),
    ("rig_palette::RigPalettes", "1164"),
    ("rig_palette::RigSkin", "1164"),
    ("rig_rider::RigRider", "1609"),
    ("schedule::WorldLive", "1160"),
    ("schedule::WorldStage", "1164"),
    ("sky_order::Rung", "wall"),
    ("terrain_stream::SPAWN_XY", "wall"),
    ("terrain_stream::ViewFocus", "1160"),
    ("terrain_stream::WorldLoadProgress", "wall"),
    ("thread_qos::QosClass", "2338"),
    ("thread_qos::ThreadQosPlugin", "wall"),
    ("thread_qos::promote_current_thread", "2338"),
    ("view::MsaaFormats", "1632"),
    ("view::MsaaSetting", "1629"),
    ("view::ViewDistance", "1164"),
    ("view::Viewer", "wall"),
    ("view::WorldCamera", "1164"),
    ("vis_chain::VisChainOnly", "1441"),
    ("weather::WeatherMessage", "1164"),
    ("weather::WeatherState", "2181"),
    ("wmo_portal::room_pvs_visible", "1475"),
    ("world_census::CensusReport", "1164"),
    ("world_census::WorldCensus", "1164"),
    ("world_map::CurrentMap", "1164"),
    ("world_map::MapChange", "1164"),
    ("world_plugins::WorldPlugins", "1164"),
    ("world_point::Subject", "1164"),
    ("world_point::WorldPoint", "1164"),
    ("world_unit::ViewerUnit", "wall"),
    ("world_unit::WorldUnit", "wall"),
    ("worldview::run", "2338"),
];

/// **The sorted leaks** — items 1164 filed to close, still crossing today, with the bucket it
/// filed them under (decision 2338 moved the sort here from prose). `CLOSE/absorb` means a facade
/// takes the call, `CLOSE/invert` the engine publishes the fact instead of the game reaching in,
/// `CLOSE/move-engine` mis-filed engine code moving engine-side (the twelve `terrain_stream::*`
/// items are `entities/wmo_props.rs`, 1164's fourth candidate, invisible until 2338's scanner
/// fix), `CLOSE/move-game` gameplay moving out of the engine. A leak that is in neither table is
/// unsorted: it crossed after 1164 and no record has said what happens to it. A row no game
/// file names any more fails the test until it is deleted — closing a leak is deleting its row.
const SORTED_LEAKS: &[(&str, &str)] = &[
    ("billboard::BillboardJointRig", "CLOSE/absorb"),
    ("billboard::BillboardPlace", "CLOSE/absorb"),
    ("billboard::billboard_basis", "CLOSE/move-engine"),
    ("billboard::billboard_joint_palette", "CLOSE/absorb"),
    ("clutter::ClutterConfig", "CLOSE/absorb"),
    ("collision::PickOccluder", "CLOSE/absorb"),
    ("decal::DecalFrame", "CLOSE/absorb"),
    ("doodad_anim::DoodadAnimTier", "CLOSE/absorb"),
    ("doodad_anim::TintAnimMaterials", "CLOSE/move-engine"),
    ("doodad_anim::UvAnimMaterials", "CLOSE/move-engine"),
    ("doodad_anim::classify", "CLOSE/absorb"),
    ("doodad_anim::sample_mat_anim", "CLOSE/invert"),
    ("doodad_anim::wants_rig", "CLOSE/move-engine"),
    ("entity_shade::GroundShade", "CLOSE/absorb"),
    ("ffx_glow::FfxDeathFade", "CLOSE/absorb"),
    ("instance_tint::IDENTITY", "CLOSE/absorb"),
    ("instance_tint::pack", "CLOSE/absorb"),
    ("interact::PickBox", "CLOSE/absorb"),
    ("interact::WorldClick", "CLOSE/move-game"),
    ("interact::WorldRightClick", "CLOSE/move-game"),
    ("interact::WorldRightPress", "CLOSE/move-game"),
    ("interact::cast_pick_ray_inflated", "CLOSE/absorb"),
    ("interior::BodyBakeCenter", "CLOSE/absorb"),
    ("interior::ContainmentAttach", "CLOSE/absorb"),
    ("interior::InteriorLit", "CLOSE/invert"),
    ("interior::InteriorReauthor", "CLOSE/invert"),
    ("interior::classify_entity_interior", "CLOSE/absorb"),
    ("interior::part_interior_lit", "CLOSE/absorb"),
    ("lighting::GameClock", "CLOSE/absorb"),
    ("lighting::PropProbeSlot", "CLOSE/move-engine"),
    ("lighting::PropProbes", "CLOSE/move-engine"),
    ("liquid::WaterChunkInfo", "CLOSE/absorb"),
    ("map_proj::WorldProj", "CLOSE/move-game"),
    ("map_proj::ZoneRect", "CLOSE/move-game"),
    ("mesh_tag::describe", "CLOSE/absorb"),
    ("mesh_tag::with_alpha", "CLOSE/invert"),
    ("model_fade::DespawnFade", "CLOSE/move-game"),
    ("model_fade::FadeMaterials", "CLOSE/absorb"),
    ("model_fade::FadeSet", "CLOSE/absorb"),
    ("model_fade::JoinedFade", "CLOSE/absorb"),
    ("model_fade::MAX_MODEL_CHAIN", "CLOSE/invert"),
    ("model_fade::PartFade", "CLOSE/absorb"),
    ("model_fade::PendingAppearFade", "CLOSE/invert"),
    ("model_fade::UnitAppearFade", "CLOSE/move-game"),
    ("model_fade::apply_render_fade", "CLOSE/absorb"),
    ("model_fade::fade_alpha", "CLOSE/absorb"),
    ("model_fade::join_unit_appear_fade", "CLOSE/absorb"),
    ("model_render::FarSideOfWater", "CLOSE/invert"),
    ("model_render::FarSideTwins", "CLOSE/invert"),
    ("model_render::ModelVisSet", "CLOSE/absorb"),
    ("model_render::ShadeSel", "CLOSE/absorb"),
    ("model_render::far_resolved", "CLOSE/invert"),
    ("model_render::replace_fog_policy", "CLOSE/absorb"),
    ("modkeys::DEV_CHORD", "CLOSE/move-engine"),
    ("modkeys::dev_chord", "CLOSE/move-engine"),
    ("particles::EmitClock", "CLOSE/absorb"),
    ("particles::EmitterFade", "CLOSE/invert"),
    ("particles::EmitterFrames", "CLOSE/absorb"),
    ("particles::OwnerLoss", "CLOSE/absorb"),
    ("particles::ParticleEmitter", "CLOSE/invert"),
    ("particles::buffer::EffectLightOverride", "CLOSE/invert"),
    ("particles::buffer::EffectQuads", "CLOSE/invert"),
    ("particles::buffer::EffectVertex", "CLOSE/absorb"),
    ("particles::buffer::WorldEffectDraw", "CLOSE/absorb"),
    ("particles::buffer::begin_effect_frame", "CLOSE/absorb"),
    ("ribbons::RibbonSeq", "CLOSE/absorb"),
    ("ribbons::RibbonTrail", "CLOSE/invert"),
    ("ribbons::spawn_ribbon", "CLOSE/absorb"),
    ("rig_palette::RigPaletteMirrors", "CLOSE/absorb"),
    ("rig_palette::RigPart", "CLOSE/absorb"),
    ("rig_palette::RigStarved", "CLOSE/invert"),
    ("rig_palette::rig_cost_enabled", "CLOSE/absorb"),
    ("terrain_stream::AreaAuthoritySet", "CLOSE/absorb"),
    ("terrain_stream::CurrentArea", "CLOSE/absorb"),
    ("terrain_stream::PendingCollider", "CLOSE/move-engine"),
    ("terrain_stream::PropLobeLight", "CLOSE/move-engine"),
    ("terrain_stream::ShadeResolve", "CLOSE/move-engine"),
    ("terrain_stream::SpawnedModel", "CLOSE/move-engine"),
    ("terrain_stream::TerrainStreamer", "CLOSE/absorb"),
    ("terrain_stream::build_collider_task", "CLOSE/move-engine"),
    ("terrain_stream::doodad_ground_shade", "CLOSE/move-engine"),
    ("terrain_stream::fold_interior_probe", "CLOSE/move-engine"),
    ("terrain_stream::hex_word", "CLOSE/move-engine"),
    ("terrain_stream::m2_anim_bound", "CLOSE/move-engine"),
    ("terrain_stream::m2_fade", "CLOSE/move-engine"),
    (
        "terrain_stream::placement_collider_data",
        "CLOSE/move-engine",
    ),
    ("terrain_stream::point_light", "CLOSE/absorb"),
    ("terrain_stream::spawn_model_entities", "CLOSE/move-engine"),
    ("view::CAM_FOVY", "CLOSE/absorb"),
    ("view::FARCLIP_RANGE", "CLOSE/absorb"),
    ("view::NEARCLIP_DEFAULT", "CLOSE/absorb"),
    ("wmo_portal::UnitWmoRoom", "CLOSE/absorb"),
    ("wmo_portal::WmoPortalInstance", "CLOSE/absorb"),
    ("wmo_portal::WmoPvsSet", "CLOSE/absorb"),
];

/// The gate — on the **leaks**. 1163 set the surface at forty; 1164 sorted it; 2338 made the
/// sort data and pointed the ratchet at what is left to close. The raise log below is the
/// history of the surface count this constant gated until then, and the source of every
/// `PUBLISHED` row that names a record or `wall`.
///
/// **Lower it when you close something.** The lower bound below exists so that a session which
/// closes twenty leaks cannot leave the ceiling standing at the old number, which would silently
/// hand the next twenty leaks a free pass.
///
/// It has been raised **twelve times**, and every reason is worth keeping. 138 → 139: `ground_fx`
/// moved into the engine, where 1164 says it belongs ("326 lines of pure render"), and the game
/// now names one entry point (`spawn_ground_fx_decal`) where before it named none — it was calling
/// a module in its own crate. A structural correction that costs one item and buys the lane's
/// ordering law back from its caller; `ModelEffects` absorbs the entry point when it is built.
///
/// **150 → 176: the crate
/// exists, and the count switched to the thing it was always a proxy for.** Two reasons it went up,
/// both of them the measurement getting more honest rather than the doorway widening:
///
/// - The module-name scan excluded the *instruments* (`debug_panel`, `perf`, `pipe_warm`) as
///   namers, on the assumption they travel with the engine. The compiler refuted that the moment
///   the move was attempted: all three read the game, so they stayed in `benilla-app` — and they
///   consume the engine API like any other consumer, which they always did.
/// - `crate::x::Y` had to be parsed out of a naming convention. `benilla_world::x::Y` is a real
///   path the compiler agrees with, so nothing hides behind a form the parser did not anticipate.
///
/// The number to work down is this one. It is also the last time it can be argued with: from here
/// every item on it is a `pub` in `benilla-world`, and closing one is a demotion the compiler
/// checks.
///
/// 149 → 150: `SPAWN_XY`, the
/// streamer's last-ditch focus, moved out of `lib.rs` and into `terrain_stream` where it is used —
/// an engine that cannot answer "where do I stream from" with no game attached is not an engine.
/// The two gameplay readers (the player's boot pose, the world viewer's start) name it across the
/// line now; it bought the one reverse crossing that neither wall could see.
///
/// 155 → 156:
/// `model_fade::ModelFade` — 1164's alpha inversion, in one component. The game declares how
/// translucent a root should be; the engine owns the write, the chain composition and the material
/// swap. One published name for two impossible dependencies, and it retires a gameplay writer of a
/// channel the reference gives one owner.
///
/// 153 → 155:
/// `world_unit::{WorldUnit, ViewerUnit}` — a body in the world and the viewer's own, replacing
/// five engine lanes' filters on the game's wire record. Two published markers for three
/// impossible dependencies (`net::NetEntity`, `net::SelfPlayer`, `entities::CollisionHeight`).
///
/// 152 → 153: `view::Viewer`
/// — the avatar as a *body* (where, how fast, which way, how tall) rather than as the game's
/// `Player` type. Three engine lanes were reading it behind the identical predicate. One published
/// resource for two impossible dependencies.
///
/// 148 → 152: `creature_anim`
/// split along 1163's line — the rig machinery (pose evaluation, world/palette composition, the
/// global-sequence channels) became `rig_anim` and the game kept the policy. The four are the real
/// "pose a rig" vocabulary (`RigPose`, `RigFrame`/`RigAnchor`, `PosePost`, `GlobalSeqDrive`), and
/// they belong beside `RigPalettes` and `RigSkin` in 1164's *place* face — a second program that
/// spawns a skinned model has to pose it. They bought three impossible dependencies.
///
/// 147 → 148: 1160's wire
/// (b). The engine stopped reading the game's four-variant session enum to learn whether a world
/// exists and started reading its own one-bit `schedule::WorldLive`, which the session writes.
///
/// 145 → 147: 1160's wire
/// (a) inverted. The streamer stopped reading `player::Player` and started reading
/// `terrain_stream::ViewFocus`, which the game writes; and `WorldLoadProgress` — the streamer's
/// own residency fact, which had been living in `loading_screen` — came home, so its two readers
/// (the bar, and the mover's post-snap hold) now name it across the line. Two published inputs
/// bought four reverse crossings, and the enforcer stopped needing a stub avatar to boot.
///
/// 143 → 145: two capture
/// fixtures that had been living inside engine modules (`water_fx::view`, the foam viewer, and
/// `particles::census`, the draw-address probe) moved out to `capture`, where they belong — and
/// an instrument on the game side naming engine internals is a forward crossing where an
/// instrument *inside* the engine was not counted at all. Same shape as the 160 → 162 raise
/// below: the doorway did not widen, the measurement got honest. It closed three reverse
/// crossings, including an engine plugin that had to name `capture` to register a fixture.
///
/// 142 → 143: the depth-bias
/// ladder came together in `sky_order` as one `Rung` type. That is a genuinely new forward
/// crossing — gameplay lanes now name an engine rung where before each kept its own constant —
/// and it bought three REVERSE crossings, the ones that cannot exist at all once the crate does.
/// One published rung against three impossible dependencies is the trade, and the ladder is
/// engine API by 1164's own spine (a second program orders against the engine's frame).
///
/// 133 → 154, and this is the one to read first: it is not a leak, it is the wall admitting it
/// had been lying. `expand` stopped walking at a brace, so `use benilla_world::interact::{A, B}`
/// — the ordinary Rust import — collapsed to the bare module path and was then discarded as a
/// prefix. Only items written out fully qualified at a *usage* site were ever counted. Every
/// number this test has printed since the crate landed was an undercount of the same kind, and
/// the honest surface is 154. Found by disagreement: a second scanner put the total one item
/// lower while naming items this one had never heard of, and neither error was visible from
/// either side alone. `WOW_API_DUMP=1` exists because of this.
///
/// And 160 → 162, the first one: moving
/// `StreamActivity` out of `perf` and `DebugState` out of `debug_panel` did not widen the doorway,
/// it stopped two engine facts hiding inside instrument modules this test deliberately does not
/// count. The items were always crossing; the measurement improved. That is the only kind of raise
/// that is not a retreat — a raise for a NEW leak is the failure this number exists to catch.
///
/// And 158 → 159: `vis_chain::VisChainOnly`, a PUBLISH by 1164's test. The
/// chain-only visibility idiom — keep `Visibility`+`InheritedVisibility` (hide-propagation),
/// remove `ViewVisibility` (the per-camera sweep row) — is an ENGINE law about bevy's visibility
/// pipeline, but half the never-rendering hierarchy nodes it applies to are spawned game-side
/// (net-object roots, held-item and spell-fx wrappers, transports). Keeping it engine-private
/// would mean every game spawner hand-rolling `.remove::<ViewVisibility>()` with its own copy of
/// the why — the exact drift a named idiom exists to prevent; the trait is the smallest honest
/// carrier of the law.
///
/// And 157 → 158: `collision::ColliderEpoch`, a PUBLISH by 1164's test. It is the
/// stamp on the world's collider set — "the geometry you last asked has changed" — and the whole
/// point of it is that a *cached* collision answer must not outlive the world it described. The
/// engine owns the fact (the streamer's attach queue is what changes the set), the game owns two of
/// the three parties: the creature ground clamp reads it, and the GameObject hull lane
/// (`entities::attach`) is the one collider insert outside the streamer's queue, so it must be able
/// to stamp. Keeping it engine-private would mean either a game-side collider that no cache can see
/// arrive — which is B197's bug with a different collider class — or an engine system reaching into
/// game components to invalidate them, which crosses this line the other way and worse.
///
/// And 156 → 157: `mat_anim_table::MatAnimTable`, a PUBLISH by 1164's test. The
/// mat-anim delta table replaced per-frame material mutation, and registration must happen where
/// materials are BUILT — which for WMO GameObject props (transport interiors) is a game-side
/// spawner (`entities::wmo_props`) that already threads the two registries this table serves
/// (`UvAnimMaterials`/`TintAnimMaterials`, published members of the same family). The resource
/// that allocates the slots is the smallest honest addition beside them; hiding it would mean
/// a game-side material registering without a slot and silently freezing at its seed.
///
/// And 155 → 156: `doodad_anim::DoodadAnimHost`, a PUBLISH by 1164's test. The
/// doodad joint collapse put placed doodads on the collapsed-rig lane (`RigPose`), which made
/// them visible to the game's animation-LOD gate — whose park marker the engine's own doodad
/// draw gate already owns, on a different law (the composed draw verdict + fade sphere, not the
/// unit frustum test). Two writers to one marker silently un-park hidden hosts, so the game's
/// gate must be able to say "this population is not mine" — and the engine component that IS
/// that population's name is the smallest honest way to say it. The instruments already named
/// it; this makes the one gameplay filter explicit rather than inventing a second marker to
/// carry the same fact.
///
/// And 154 → 155: `modkeys::SyntheticHold`, a PUBLISH by 1164's test rather than a leak. The
/// engine owns the macOS stuck-modifier reconciler, and that reconciler decides by polling the
/// **hardware** flag state — which by construction reads "up" for a key no hand is on. So a
/// synthesized press (the probe harness's `WOW_PROBE_KEY`) was released the frame after it was
/// made, and logged as a stuck-key correction: every chord binding was silently unreachable
/// headlessly on the platform we develop on. "Something is deliberately holding this key" has
/// exactly one reader — the reconciler, engine-side — and its writers are whoever synthesizes
/// input, which is game-side; a one-field resource is the smallest honest expression of that. The
/// alternative was ordering a game system against an engine system, which crosses this same line
/// *and* publishes a schedule point to do it.
/// And 159 → 160: `WorldLoadProgress::gx_pending`, a PUBLISH beside the two residency terms
/// already through this door (`colliders_pending`, `merge_pending`). The retained pass (1429)
/// moved the static world off the entity path, so a placement that has SPAWNED still draws
/// nothing until its region bakes — and the game reads residency for two decisions the engine
/// does not own: when the loading cover may lift, and when the post-snap physics hold may
/// release (decision 0737's split). Both were answering "is the world there" with a fact that
/// had stopped meaning it. The alternative was the game reaching into
/// `StaticGx` itself, which is a whole engine subsystem through the door instead of one
/// `usize` on the residency struct that exists to be read from outside.
/// And 160 → 161: `mac_quit::MacQuitPlugin`, a PUBLISH in the exact class of the two engine
/// plugins already through this door — `thread_qos::ThreadQosPlugin` and `bgwin::BgWinPlugin`.
/// All three correct a *host platform default* at launch, both composition roots name all three,
/// and they sit outside `WorldPlugins` because they are properties of the process and its window
/// rather than of the world. This one re-points macOS's `Cmd+Q` (winit's default menu wires it
/// straight to `terminate:`, which exits without ever running another frame) at the window close,
/// so the client's shutdown tail gets the frame it needs — decision 1528, where that gesture was
/// measured on the real client writing *nothing*: no saved variables, no addon files, no camera
/// pose. The alternative was hiding it inside `WorldPlugins` to keep the count flat, which is
/// gaming the instrument rather than paying for a name.
/// And 161 → 162: `particles::ViewThrottled`, a PUBLISH of the same shape as
/// `rig_anim::AnimParked` already through this door — a marker the ENGINE reads and the GAME
/// writes, which is the 1160 line drawn exactly where 1163 drew it. Freezing an emitter with the
/// view that stopped drawing it is machinery (the draw-set law lives in `particles::sim`);
/// deciding that a particular booth camera is being *paced* rather than put to sleep is policy,
/// and it is `portrait::gate_booth_cameras`'s, in the game. There is no engine-side way to tell
/// the two apart — a camera's `is_active` bit says only that it did not draw — which is precisely
/// the conflation that ran the body panes' item effects at half speed. The
/// alternative was passing a `Res<PaneRate>`-shaped opinion down into the engine's own sim, which
/// puts the game's knob inside the renderer to keep a count flat.
/// And 162 → 163: `rig_rider::RigRider`, a PUBLISH of the same shape as `rig_anim::AnimParked`
/// and `particles::ViewThrottled` above — a component the ENGINE reads and the GAME writes.
/// Placing an attached model's vertices in its wearer's rig frame rather than in absolute f32
/// world space is machinery (the whole lane is `rig_rider` + the palette's row writer, and the
/// reason is arithmetic: at Elwynn's ~9481 yards the absolute sum lands on an 0.98 mm grid and is
/// rebuilt every frame). *Which* bone a helm hangs from, and what a sheath swap does to that, is
/// policy, and it is `entities::equipment`'s, in the game — the same M2 attachment table that
/// decides where a weapon is drawn versus stowed. There is no engine-side way to know it: the
/// engine sees a scene-graph child, and a child is not an attachment. The alternative was passing
/// the game's attach-slot table down into the renderer to keep a count flat.
/// And 163 → 164: `instance_tint::InstanceTintMirrors`, the twin of `rig_palette::RigPaletteMirrors`
/// already through this door and registered at exactly the same site — an off-world `wow_light`
/// buffer the engine must also fill with the per-instance tint region, because the shader reads
/// that region out of whichever buffer the draw binds. Uploading a region to the buffers that
/// carry it is machinery. *Which* off-world buffers carry it is policy, and it has to be: a
/// portrait bake must NOT (the reference builds a fresh CM2 with colour `(1,1,1)`, so a ghost's
/// portrait shows the living face — `0x524f60`, report B49)
/// while the glue scene MUST (it is the screen itself, and its character component is the very
/// instance the reference tints). The engine cannot tell those two render targets apart — both are
/// a camera writing to an image — and encoding "a bake standing in for a UI model widget" inside
/// the uploader is exactly the game opinion the wall exists to keep out. The alternative was
/// mirroring the region into every registered buffer to keep the count flat, which re-opens B49 in
/// the tint lane.
/// And 164 → 165: `ffx_glow::GlueFfx`, a PUBLISH of the same shape as `rig_anim::AnimParked` and
/// the two above — a resource the ENGINE reads and the GAME writes, and exactly ONE bit wide.
/// Running an FFX pass pair is machinery; *which* pair a screen installs is the screen's own, and
/// the reference says so by building two pairs (WorldFrame `0x481c46`, CGlueMgr `0x46a723`) and
/// selecting between them at two different sites. The world's selector is already inside the
/// engine's reach — `PLAYER_FLAGS_GHOST` on the live player — but the glue's is not and cannot be:
/// it is the ghost bit of the **selected roster row** (`0x472fd9 test dh,0x20`), a character
/// nobody is standing in, on a screen the engine has no concept of. The alternative was handing the
/// renderer a roster to read to keep a count flat. Note the direction of travel:
/// this replaced `FfxGlow::state_scale`, a float that let a bake inherit a player-state lane by
/// arithmetic — the enum that took its place makes report B49's invariant structural.
/// And 165 → 166: `collision::MoverTraceExclusions`, a PUBLISH in the same class as the two
/// one-bit resources above — a resource the ENGINE reads and the GAME writes. The reference's
/// world trace carries a per-trace mask, and the local mover's gains bit `0x8000` when the body it
/// drives is a player in ghost form; the GameObject collision-candidacy virtual `0x5f85f0` reads
/// it and drops every `GAMEOBJECT_TYPE_ID == 0` object, so a ghost walks through closed doors.
/// The engine cannot compute that set — "a DOOR GameObject, while the player is a
/// ghost" names two gameplay concepts, which is precisely what the wall pointing the other way
/// forbids an engine file from knowing. The alternatives were both worse: dropping or re-laning
/// the door's collider makes it stop existing for the particle snap, the precipitation probe, the
/// mouse pick and the creature conform, none of which the binary touches; and threading a filter
/// through `step`/`grounded_step`/`airborne_step` puts a gameplay argument into the movement
/// core's signature. An `EntityHashSet` that is empty on every living frame is the smallest honest
/// expression, and it makes the living player's query provably the one it always was.
/// And 166 -> 172: **not six crossings — one measurement correction.** Nothing moved through the
/// doorway; the scan was blind to part of every file. A `#[cfg(test)]` was cleared only on
/// `depth < d`, and a top-level test module is seen at depth 0, rises to 1 and returns to 0 —
/// never below it, so the first test module in a file switched the scan off for the rest of that
/// file. [`paths_in`] carries the fix and how it surfaced (2026-09-01, a change that only *moved*
/// a test module out of `net.rs` and pushed the count up by one with no new reference in its
/// diff). These six were named by gameplay code all along, below a test module, uncounted:
/// `lighting::WorldTime`, `art_scope::ArtScope`, `art_scope::ArtSlot`,
/// `billboard::billboard_joint_palette`, `rig_anim::finalize_rig_worlds`, `world_unit::ViewerUnit`.
/// Raising the number is what keeps the ratchet honest — a ceiling measured through a hole is not
/// a ceiling — and it is the ratchet's own instruction: a count that cannot see the whole file
/// cannot report a widening either, which is the one thing this test exists to do. The six are
/// **not** hereby blessed as PUBLISH: they are un-sorted 1164 work, now visible enough to sort.
/// And 172 → 173: `model_render::lazy::realize`, a PUBLISH. A built `WowModelMaterial` is no
/// longer an asset the moment a spawner builds it — the engine parks the value behind a
/// reserved handle and inserts the asset the first frame something visible is bound to it
/// (9.9k materials, 20.7k buffers and 10.2k bind groups were alive at the
/// Stormwind auction house for 36 drawn entity batches, the variant set every spawner is handed
/// and never switches to). A game lane that CLONES a material before anything binds it — the
/// portrait booth relighting a part's twins onto its own light buffer, a spell kit deriving its
/// per-instance tinted copy from the shared steady — asks the engine to realize it first. That
/// is the whole of the crossing: one function, "make this handle's asset exist now", the
/// doorway's own rule about when a material is real. The alternative — eager materials for
/// the two lanes that read early — would put the store's law in the caller's hands.
/// And 173 → 174: `particles::render::EFFECT_DRAW_STATS`, an instrument PUBLISH — the effect
/// lane's own per-frame draw census (items in the transparent phase, draws after the merge),
/// the number the `FPS_PROBE` line prints as `fx=`. It is the reading that refuted the
/// additive-window regroup on the crowd rig: a lane whose merge walk is the
/// only place the count exists has to publish it or stay unmeasurable.
/// And 174 → 175: `dev_state::STILL_INPUTS_CHANGED`, an instrument PUBLISH — how many frames
/// each whole-scene input of the still-frame skips (1979) read as changed. Counted in the
/// world crate so the probe names ONE static instead of the four resources the skips read; the
/// reading is what says whether a gate ever engages.
/// And 175 → 176: `mat_anim_table::affine_row`, a PUBLISH. The mat-anim table's second row
/// kind: a texture transform's rotation and scale as deltas from the identity,
/// `[cos − 1, sin, sx − 1, sy − 1]`, so that row 0 — the pinned zero every static material
/// reads — IS the identity. That encoding is the table's own law (the same zero-is-identity
/// rule its translation rows and the tint table run under), and the shader's fold is written
/// against it; a lane that owns rows in the table — the UI model tiles, sampling the cooldown's
/// quadrant rotations off the pane's play head — has to write them in the table's encoding,
/// not one of its own. One function, "encode this affine the way the table reads it".
/// And 176 → 177: `mat_anim_table::MatAnimMirrors`, a PUBLISH — the mat-anim table's twin of
/// `instance_tint::InstanceTintMirrors`. A lane whose materials bind a light
/// buffer of their own reads the table out of THAT buffer, so the UI model tiles' rows — the
/// cooldown sweep's rotations, written every frame off the pane's play head — reached a region
/// nothing in a tile ever sampled until the tile's buffer was on a mirror list. Registered once
/// at the tile renderer's startup, the same shape and the same reason as the palette mirror
/// beside it.
/// And 177 → 178: `model_fade::UnitRenderAlpha`, a PUBLISH — one streamed unit's live render
/// alpha, composed from the unit ROOT's own presentation state. `ModelAlphas` beside it is the
/// read side for a consumer that can wait for `PostUpdate`'s publish; this is the read side for
/// one that cannot — the blob shadow runs in `Update`, and on the frame a unit's presentation
/// begins the published component does not exist yet. Published as a door rather than as its
/// three inputs (`UnitAppearFade`, `DespawnFade`, `model_render_alpha`) precisely because the
/// lane that reconstructed the answer from parts got it wrong the same way every time: it read
/// "no part is fading" as opaque, which is false of a *pending* unit, and put a full-strength
/// shadow on the ground under an invisible creature for the length of a load. A caller asking
/// one question cannot make that mistake; a caller handed the inputs can.
/// And 178 → 179: `interior::NodeAmbient`, a PUBLISH — one light node's committed **ambient word
/// alone**, the ramped chase toward `cap96(MOCV)`. `ParticleLight` beside it is the same words
/// folded into the whole fixed-function term (`ambient + 0.9·diffuse + Σ lamps`), which is what a
/// lit particle quad receives; this is the read side for a draw whose vertex format carries **no
/// normal**, so the normal array is disabled outright and its term is the ambient product and
/// nothing else (the weapon swing trail). Published rather than reconstructed
/// for the reason the entry above gives: the caller cannot rebuild it from `ParticleLight` — the
/// diffuse lobe and the MOLT points are already summed in and cannot be subtracted back out — and
/// a caller that reached for the *scene* ambient instead, which is what this replaces, tinted
/// every indoor trail with the sky.
/// And 179 → 180: `weather::WeatherState`, a PUBLISH — the weather driver's own state, reached by
/// `crate::cvars` because one byte of it (`weather_density`) is a **player setting** and 2181 gave
/// it its switch. It is the same shape as `clutter::ClutterConfig` two rows of that table up: a
/// resource the engine owns and the options window writes exactly one field of, welded to its CVar
/// by `registered_defaults_mirror_the_code_truths`. Published as the state rather than as a
/// density-only door for the reason the two entries above give in reverse — there is no
/// reconstruction to get wrong here, the field is a `u8` the engine reads directly, and a
/// one-field wrapper would be a second name for the same byte that the weld test would then have
/// to hold in step with both sides. It was already on the far side of the wall as an INSTRUMENT
/// item (the `#[cfg(feature = "dev")]` weather panel is its only other namer); what changed is
/// that a game module names it now, which is exactly the crossing this gate exists to make
/// visible.
/// And 180 → 181: `final_pass::FinalPassTarget`, a PUBLISH — where a colour lane's final pass
/// lands, as one noun. The client has two colour lanes that end in a full-screen
/// decode: the world's (the FFXGlow combine, engine-side) and the UI's (`crate::ui_gamma`'s,
/// game-side since 0254 — the interface is the game's). Both used to write bevy's main texture
/// and let its `upscaling` blit copy the result out; 2206 has each render straight into its
/// camera's target when the camera's output mode is `Skip`, and the rule that turns an output
/// mode into a destination, a format and a scissor is one rule, not two copies of it that drift.
/// It could not go the other way: moving the UI decode into the engine would put the UI lane's
/// colour law on the wrong side of the wall. So the engine publishes the rule as one type with
/// two associated functions, and the game names it once.
/// And 181 → 182: `ffx_glow::FfxBackdrop`, a PUBLISH — the component that makes the world's
/// FFX combine the first draw of the player-UI camera's main pass. The combine is engine-side
/// (the world lane's byte math); the camera it now runs on is the game's (the interface,
/// 0254); so the engine publishes the claim as one component the game puts on its camera and
/// points at the world camera it owns, and the two nodes behind it stay private. It retires a
/// full-window float image that one camera wrote and the next read back — the seam 1603 built
/// and 2215 measured — and it could not go the other way for 2206's reason: the UI camera cannot
/// move into the engine.
/// And 182 → 183: `doodad_anim::register_fx_uv`, a PUBLISH — put one spell-effect material clone
/// on the per-instance UV-scroll lane. The clone is the GAME's: `entities::spell_fx`
/// makes it because one cast is one phase (the same reason 0271's animated tint clones it), and
/// nothing engine-side knows an effect instance exists. Everything after that is the engine's —
/// which registry, which delta-table row, which of 1408's two baked loop spellings, and the
/// instance-clock law that separates this lane from the shared one. Published as ONE verb rather
/// than as its three pieces (`UvLoop`'s effect variant, `register_uv`, the row) precisely because
/// this lane's recurring bug is a caller that takes some of the pieces and not the rest: 2038's
/// marked-but-unregistered parts froze every waterfall in the game for three days, and a
/// registration without a row, or a row without a registration, is that same shape. One verb
/// cannot be half-taken. It is also why the three pieces went back to `pub(crate)` in the same
/// commit, which is the rare crossing that *lowers* the surface it replaces.
/// And 183 → 184: `doodad_anim::UvLoops`, the argument of the verb above. A texture transform is
/// ONE authored record with four baked channels — 1408's two translation spellings plus 2019's
/// per-slot rotation and scaling — and which of them a batch fills is not a thing the caller gets
/// to reason about: a scale-only transform (`Spells\GroundingTotem_Impact.mdx`) and a
/// dead-slot-0 translation both answer `None` to the channel you would check first. So the struct
/// carries the record whole, and `any()`/`open_offset()` on it are the two questions the game
/// asks — which keeps the "does this batch animate" predicate and the "what does it open on" seed
/// in the engine, where the bake's rules live, instead of copied into the effect attach where they
/// would drift. Four positional `Option`s would have been the alternative, and swapping two of
/// them is a silent wrong-channel bug the compiler cannot see.
/// And 184 → 186, both for the ENTITY lane, which is the same crossing 2282 made
/// for the effect lane and made for the same reason.
/// `doodad_anim::register_entity_uv` is the second PUBLISH verb on this lane: put a unit /
/// GameObject / held-item batch material on the UV lane, picking the shared clock or the
/// instance's play head from the authored record rather than making the caller reason about it.
/// It is a separate verb from `register_fx_uv` and not a flag on it because the two lanes differ
/// in the one thing a caller cannot get right by accident — an effect's clocks are measured from
/// its own attach and a resident entity's are the scene's — and a boolean spelling of
/// that would read as a preference.
/// `model_render::EntityUvLane` is the argument `entity_variants` grew, and the crossing it buys
/// is the point of the decision: building an entity batch's material and putting it on the lane
/// are ONE act, so the engine takes the registry and the delta table *in the same call* instead of
/// handing back six handles and trusting the game to register them. That is 2038's law spelled in
/// the signature — a `play_uv` flip without a registration freezes every entity batch at its first
/// key instead of its identity, which is a different wrong frame and not a fix — and it is why the
/// lane is a bundle rather than three arguments a caller can pass two of.
/// And 186 → 189, the second half of 2295 — the ONE entity population that needs a material of
/// its own rather than the batch's, which the dressing path clones and registers because the
/// clone has to exist before the part's interior and fade records are built from it.
/// `doodad_anim::AnimMatPart` is the draw-scan marker, and it crosses for the reason 1375 put it
/// at the spawn site in the first place: the marker and the registration are one predicate, and
/// the game is where an entity part is spawned. The engine cannot insert it — it does not know
/// an entity part exists — and a lane that let the two drift is 2038's three frozen days.
/// `doodad_anim::register_tint` and `doodad_anim::TintLoop` are the tint channel's twins of the
/// UV verbs above, for the same five batches on three GameObject models: `G_FreezingTrap`'s glow
/// card, `OrgrimmarPentagram`, `ScholomanceCrystalBall01` — four of whose five batches bake
/// **nothing** in file slot 0, so a shared material can only ever seed white however faithfully it
/// is ticked. They were already `pub` for the world streamer; what crosses here is the game naming
/// them, and it is the same crossing the UV half makes one line up.
/// And 189 → 190: `terrain_stream::CurrentArea`, taken by `capture/probe_bg.rs`.
/// It is the engine's own area authority — the `AreaTable.dbc` leaf under the player's feet, which
/// the engine already publishes for its own audio and zone-text consumers — and the probe reports
/// it for one reason: a battleground census has to be able to say *where the body actually is*,
/// and a map id alone cannot. "Map 489" is true of the pen, the field and the graveyard alike; the
/// area id is what told this instrument that the port lands at Silverwing Hold and that the
/// release lands at the graveyard. The alternative was to re-derive the leaf in the probe from
/// tiles the engine already resolved, which is the copy-the-rule drift this wall exists to stop.
///
/// **190 → the leaks.** The surface stood at 190 published-and-leaked items
/// together; the tables above split it, `capture` joined the instruments as 1164 always counted
/// it, and the scanner learned to read a `use` group that spans lines (nine imports, twenty-one
/// items counted nowhere until then — see [`logical_lines`]). What this constant gates from here
/// is the leak count alone, measured on the tree that lands.
const LEAK_CEILING: usize = 107;

/// How far under [`LEAK_CEILING`] the real count may sit before this test asks for the ceiling
/// to be lowered. Slack, not tolerance: it keeps a single closure from failing the gate, while
/// making it impossible to bank a whole stage of work without writing the new number down.
const LEAK_SLACK: usize = 10;

#[test]
fn the_world_api_doorway_stays_shut() {
    let all = measure();
    // Partition one: an item only an instrument names is the instruments' surcharge, not the
    // designed doorway (see `INSTRUMENT_CONSUMERS`). One a game module *also* names is the doorway's.
    let (probes, surface): (BTreeMap<_, _>, BTreeMap<_, _>) = all
        .into_iter()
        .partition(|(_, files)| files.iter().all(|f| is_instrument_consumer(f)));
    // Partition two: the doorway is the published API plus the leaks, and only
    // the leaks gate.
    let published_by: BTreeMap<&str, &str> = PUBLISHED.iter().copied().collect();
    let sorted_by: BTreeMap<&str, &str> = SORTED_LEAKS.iter().copied().collect();
    let both: Vec<&str> = published_by
        .keys()
        .filter(|k| sorted_by.contains_key(*k))
        .copied()
        .collect();
    assert!(
        both.is_empty(),
        "an item cannot be both published and a sorted leak: {both:?}"
    );
    let (published, leaks): (BTreeMap<_, _>, BTreeMap<_, _>) = surface
        .into_iter()
        .partition(|(k, _)| published_by.contains_key(k.as_str()));
    let n = leaks.len();
    let sorted = leaks
        .keys()
        .filter(|k| sorted_by.contains_key(k.as_str()))
        .count();
    // Always say the numbers: `cargo test -p benilla-app --test world_api_wall -- --nocapture`
    // is the one-command answer to "where is the wall now".
    eprintln!(
        "world API surface: {} items — {n} leaks (ceiling {LEAK_CEILING}, slack {LEAK_SLACK}; \
         {sorted} sorted by 1164, {} unsorted), {} published; + {} named only by the instruments",
        published.len() + n,
        n - sorted,
        published.len(),
        probes.len()
    );

    // The tables are the sort. A row no game file names any more is a row to delete — a stale
    // published row would let the item cross again for free, and a stale leak row is a closure
    // nobody wrote down.
    let stale: Vec<&str> = PUBLISHED
        .iter()
        .map(|(k, _)| *k)
        .filter(|k| !published.contains_key(*k) && !probes.contains_key(*k))
        .chain(
            SORTED_LEAKS
                .iter()
                .map(|(k, _)| *k)
                .filter(|k| !leaks.contains_key(*k) && !probes.contains_key(*k)),
        )
        .collect();
    assert!(
        stale.is_empty(),
        "rows in PUBLISHED / SORTED_LEAKS that no game file names any more — delete them (a \
         closed leak is a deleted row; a published item nobody uses is not API):\n  {}",
        stale.join("\n  ")
    );

    // `WOW_API_DUMP=1` prints the surface itself, most-named first, each item with what the
    // tables say about it. The count alone answers "is the wall holding"; it cannot answer
    // "which item did that unit actually retire", and a unit that reads as net-zero is exactly
    // when you need the list.
    if std::env::var("WOW_API_DUMP").is_ok() {
        let tagged = |m: &BTreeMap<String, BTreeSet<String>>, tag: &dyn Fn(&str) -> String| {
            let mut rows: Vec<_> = m
                .iter()
                .map(|(k, v)| (v.len(), format!("{k}  [{}]", tag(k))))
                .collect();
            rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            rows.iter()
                .map(|(n, k)| format!("  {n:3} file(s)  {k}"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        eprintln!(
            "--- leaks ---\n{}\n--- published ---\n{}\n--- named only by the instruments ---\n{}",
            tagged(&leaks, &|k| sorted_by
                .get(k)
                .map_or_else(|| "unsorted".to_string(), |b| b.to_string())),
            tagged(&published, &|k| published_by[k].to_string()),
            tagged(&probes, &|k| published_by.get(k).map_or_else(
                || "instrument".to_string(),
                |b| format!("published {b}")
            )),
        );
    }

    if n > LEAK_CEILING {
        let fresh: Vec<_> = leaks
            .iter()
            .filter(|(k, _)| !sorted_by.contains_key(k.as_str()))
            .map(|(k, v)| (v.len(), k))
            .collect();
        panic!(
            "world API leaks: {n}, ceiling {LEAK_CEILING}.\n\
             Something new crossed the line and no record publishes it. Either close it, or — if \
             it is genuine engine API — add it to PUBLISHED with the record that says so (decision \
             2338); never a bare number. The unsorted leaks on this tree:\n\n{}",
            render(&fresh)
        );
    }
    assert!(
        n + LEAK_SLACK >= LEAK_CEILING,
        "world API leaks are down to {n} and the ceiling still says {LEAK_CEILING}. Lower \
         LEAK_CEILING to {n} in this file so the next leak has to earn its place — the ratchet \
         only holds if the number follows the work down."
    );
}

/// **The wall that points the other way.** 1160 stage one's other half: an engine file naming a
/// gameplay item is a dependency `benilla-world` cannot express, so every one of these has to go
/// before the crate can exist at all. Ratcheted to zero, and then the crate graph keeps it there.
#[test]
fn the_engine_names_nothing_of_the_game() {
    let surface = measure_back();
    let n = surface.len();
    eprintln!("engine→game surface: {n} items (target 0 — CLOSED)");
    assert!(
        n == 0,
        "the engine names {n} gameplay items, and this wall is CLOSED — it reached zero and must \
         stay there.\n\
         An engine file reaching into the game is the dependency `benilla-world` cannot have: not \
         a lint, a compile error waiting for the crate to exist. Invert it (the engine publishes \
         the fact, the game reads it), move the caller, or move the thing named.\n\n{}",
        render(
            &surface
                .iter()
                .map(|(k, v)| (v.len(), k))
                .collect::<Vec<_>>()
        )
    );
}

/// Every distinct engine item named by non-engine code, and the files that name it.
fn measure() -> BTreeMap<String, BTreeSet<String>> {
    let app = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let split = app
        .parent()
        .is_some_and(|c| c.join("benilla-world").is_dir());

    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let src = app.join("src");
    let tests = test_module_files(&src);
    for file in rs_files(&src) {
        let rel = file
            .strip_prefix(&src)
            .unwrap()
            .to_string_lossy()
            .to_string();
        // Pre-move, the caller's own module decides which side of the line it is on; post-move,
        // every file in this crate is on the game side by construction.
        if (!split && is_engine(&rel)) || tests.contains(&rel) {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        for (path, _) in paths_in(&text, split, true) {
            out.entry(path).or_default().insert(rel.clone());
        }
    }
    drop_module_paths(out)
}

/// Drop every entry that is only the **path to** another entry.
///
/// `use benilla_world::particles::buffer;` exists so the file can write `buffer::EffectQuads`
/// below, and that second form is the one worth counting. There is no syntactic way to tell a
/// module path from a free function — `particles::buffer` and `mesh_tag::spawn_tag` look
/// identical — but there is a structural one: a module path is a **strict prefix of something
/// else that was named**, and a function is a prefix of nothing.
///
/// This generalises a special case that only caught single-segment modules and so missed
/// `particles::buffer` by one level. A rule that needs a list of exceptions is usually the wrong
/// rule; this one needs none.
fn drop_module_paths(
    all: BTreeMap<String, BTreeSet<String>>,
) -> BTreeMap<String, BTreeSet<String>> {
    let keys: Vec<String> = all.keys().cloned().collect();
    all.into_iter()
        .filter(|(k, _)| {
            let prefix = format!("{k}::");
            !keys.iter().any(|other| other.starts_with(&prefix))
        })
        .collect()
}

/// Every **gameplay** item named by an ENGINE file — the wall that points the other way.
///
/// Empty by construction once the crate exists: `benilla-world` does not depend on `benilla-app`,
/// so a name that crosses this way is not a lint, it is a compile error. Before the move it is
/// only a convention, and this is what holds it.
fn measure_back() -> BTreeMap<String, BTreeSet<String>> {
    let app = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if app
        .parent()
        .is_some_and(|c| c.join("benilla-world").is_dir())
    {
        return BTreeMap::new(); // the crate graph is the enforcement now
    }
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let src = app.join("src");
    let tests = test_module_files(&src);
    for file in rs_files(&src) {
        let rel = file
            .strip_prefix(&src)
            .unwrap()
            .to_string_lossy()
            .to_string();
        if tests.contains(&rel) {
            continue;
        }
        // Instruments are allowed to see both sides — 1160 puts them at the top of the stack, so
        // they are not part of the engine being extracted and their reads are a later record's.
        let root = rel.split(['/', '.']).next().unwrap_or("");
        if !is_engine(&rel) || INSTRUMENT_ROOTS.contains(&root) {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        for (path, _) in paths_in(&text, false, false) {
            out.entry(path).or_default().insert(rel.clone());
        }
    }
    out
}

/// Every source file that is a **test module in its own file** — declared `#[cfg(test)] mod x;`
/// somewhere, with its body in `x.rs` or `x/mod.rs` beside its parent.
///
/// 1164's counting rule says `#[cfg(test)]` bodies do not count: a test naming an internal is not
/// an API consumer. The scan honours that for an inline `#[cfg(test)] mod tests { … }` by tracking
/// brace depth, and used to miss it entirely when the same module lives in its own file — so a
/// test file inflated the wall with items no shipping code names. One item was hiding behind this
/// (`mesh_tag::alpha_of`, read by `aura_visual/tests.rs` to assert a packed alpha round-trips);
/// the rule, not the count, is why it is worth fixing.
fn test_module_files(src: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for file in rs_files(src) {
        let text = std::fs::read_to_string(&file).unwrap();
        // A module file's children live in a directory named after it — `a.rs`'s `mod b;` is
        // `a/b.rs`; `a/mod.rs`'s is `a/b.rs` too.
        let dir = match file.file_name().and_then(|n| n.to_str()) {
            Some("mod.rs") | Some("lib.rs") | Some("main.rs") => {
                file.parent().unwrap().to_path_buf()
            }
            _ => file.with_extension(""),
        };
        let mut armed = false;
        for line in text.lines() {
            let t = line.trim();
            if t == "#[cfg(test)]" {
                armed = true;
                continue;
            }
            if armed {
                if let Some(name) = t
                    .strip_suffix(';')
                    .and_then(|d| d.rsplit_once("mod "))
                    .map(|(_, n)| n)
                {
                    for cand in [
                        dir.join(format!("{name}.rs")),
                        dir.join(name).join("mod.rs"),
                    ] {
                        if cand.is_file() {
                            out.insert(
                                cand.strip_prefix(src)
                                    .unwrap()
                                    .to_string_lossy()
                                    .to_string(),
                            );
                        }
                    }
                }
                armed = false;
            }
        }
    }
    out
}

fn is_engine(rel: &str) -> bool {
    let root = rel.split(['/', '.']).next().unwrap_or("");
    ENGINE_ROOTS.contains(&root)
}

/// Walk a source file and yield `(canonical item, line)` for every engine path it names in code.
///
/// Hand-rolled rather than a regex dependency, and deliberately literal: it follows `crate::root::`
/// (or `benilla_world::` once the crate exists), expands a `use` brace group so
/// `use crate::assets::{AssetSet, LockRecover}` counts as two items, and stops the path at its
/// first capitalised segment so `crate::assets::WorldAssets::get` is one item and not two.
fn paths_in(text: &str, split: bool, want_engine: bool) -> Vec<(String, usize)> {
    let prefix = if split { "benilla_world::" } else { "crate::" };
    let mut found = Vec::new();
    let mut depth: i32 = 0;
    // **A `#[cfg(test)]` guards exactly the item that follows it, and the scan has to find that
    // item's END.** It did not, and the miss was total: the guard was cleared only on `depth < d`,
    // but a top-level `#[cfg(test)] mod tests { … }` is seen at depth 0, rises to 1 inside and
    // returns to 0 — never *below* it. So the first test module in a file switched this scan off
    // for the whole rest of that file, and every engine item named below it went uncounted. The
    // wall's whole job is to be a number nobody has to re-measure, and it was measuring a prefix.
    //
    // Found on 2026-09-01 by a change that only *moved* a test module out of `net.rs`: the count
    // rose by one with no new reference anywhere in the diff, because deleting the module handed
    // the scan back the rest of the file. An instrument that moves when the code does not is
    // reporting on itself.
    //
    // The extent, for both shapes that exist here: `test_at` arms on the attribute line at depth
    // `d`; if the guarded item opens a brace (`mod tests {`, `impl X {`) the depth rises and the
    // item ends when it comes back to `d`; if it opens none (`#[cfg(test)] mod tests;`,
    // `#[cfg(test)] pub(crate) use …;`) it ends at that statement's semicolon.
    let mut test_at: Option<i32> = None;
    let mut test_open = false;
    for (n, line) in logical_lines(text).iter().map(|(n, l)| (*n, l.as_str())) {
        let t = line.trim_start();
        if t.starts_with("#[cfg(test)]") {
            test_at = Some(depth);
            test_open = false;
        }
        let skip = test_at.is_some() || t.starts_with("//") || t.starts_with('*');
        if !skip {
            let mut rest = line;
            while let Some(i) = rest.find(prefix) {
                // A bare `xcrate::` is not our prefix; require a non-ident char before it.
                let boundary = i == 0 || !is_ident(rest.as_bytes()[i - 1] as char);
                let tail = &rest[i + prefix.len()..];
                if boundary {
                    // A **crate-root** item — `crate::SPAWN_XY`, no module segment — is neither
                    // side's by module, and both walls were structurally blind to it: the parse
                    // below wants a lowercase module root, so an uppercase first segment fell out
                    // entirely. It belongs to the *game* by construction, because `lib.rs` is what
                    // stays behind in `benilla-app` when the engine moves — so an engine file
                    // naming one is a reverse crossing, and it took a hand-rolled grep over the
                    // engine file set to find the one that was hiding (`SPAWN_XY`).
                    let root_item = !split
                        && ident(tail).is_some_and(|(r, a)| {
                            r.starts_with(char::is_uppercase) && !a.starts_with("::")
                        });
                    if root_item && !want_engine {
                        if let Some((r, _)) = ident(tail) {
                            found.push((format!("crate::{r}"), n + 1));
                        }
                        rest = &rest[i + prefix.len()..];
                        continue;
                    }
                    let (root, after) = if split {
                        (String::new(), tail)
                    } else {
                        match ident(tail) {
                            Some((r, a))
                                if a.starts_with("::") && !r.starts_with(char::is_uppercase) =>
                            {
                                (r, &a[2..])
                            }
                            _ => (String::new(), ""),
                        }
                    };
                    // Three sides, not two. An INSTRUMENT is neither: it is excluded from the
                    // doorway being measured (1160 puts instruments at the top of the stack), and
                    // an engine file naming one is not a reverse dependency either — every
                    // instrument module is registered by `WorldPlugins`, so it travels *with* the
                    // engine into the crate.
                    let instrument = !split && INSTRUMENT_ROOTS.contains(&root.as_str());
                    let engine = split || (!instrument && ENGINE_ROOTS.contains(&root.as_str()));
                    let keep = if want_engine {
                        engine
                    } else {
                        !engine && !instrument
                    };
                    if keep && !after.is_empty() {
                        for tail in expand(after) {
                            let full = if root.is_empty() {
                                tail
                            } else {
                                format!("{root}::{tail}")
                            };
                            let item = canon(&full);
                            // A bare module name is not an item — `use benilla_world::interact;`
                            // exists so the file can write `interact::WorldObject` below. Most of
                            // these are caught structurally by `drop_module_paths`, but not the
                            // ones whose member is only named where this scan does not look (a
                            // `#[cfg(test)]` block, a macro body), so the parse drops them too.
                            let bare_module =
                                !item.contains("::") && !item.starts_with(char::is_uppercase);
                            if !bare_module {
                                found.push((item, n + 1));
                            }
                        }
                    }
                }
                rest = &rest[i + prefix.len()..];
            }
        }
        depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
        if let Some(d) = test_at {
            if depth > d {
                test_open = true;
            } else if test_open || t.trim_end().ends_with(';') {
                test_at = None;
                test_open = false;
            }
        }
    }
    found
}

/// The file's lines, with a `use` statement whose brace group spans several physical lines
/// joined into one. The scan is line-based, and [`expand`] given a `{` with nothing after it
/// yielded nothing — so every member of such a group was invisible to both walls: nine imports
/// in game files, twenty-one items counted nowhere, among them a 1164 CLOSE row and the twelve
/// `terrain_stream::*` names `entities/wmo_props.rs` reaches for. Each joined
/// line keeps its first physical line's number; a `//` comment inside the group is cut at the
/// comment (a `use` statement has no string literal a `//` could sit in).
fn logical_lines(text: &str) -> Vec<(usize, String)> {
    fn open(s: &str) -> i32 {
        s.matches('{').count() as i32 - s.matches('}').count() as i32
    }
    fn strip_comment(s: &str) -> &str {
        s.find("//").map_or(s, |i| &s[..i])
    }
    let mut out = Vec::new();
    let mut lines = text.lines().enumerate();
    while let Some((n, line)) = lines.next() {
        let t = line.trim_start();
        let is_use = t.starts_with("use ")
            || (t.starts_with("pub") && t.contains(" use ") && !t.contains('='));
        if is_use && open(strip_comment(line)) > 0 {
            let mut joined = strip_comment(line).trim_end().to_string();
            let mut depth = open(&joined);
            for (_, next) in lines.by_ref() {
                let next = strip_comment(next);
                joined.push(' ');
                joined.push_str(next.trim());
                depth += open(next);
                if depth <= 0 {
                    break;
                }
            }
            out.push((n, joined));
        } else {
            out.push((n, line.to_string()));
        }
    }
    out
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The leading identifier of `s`, and what follows it.
fn ident(s: &str) -> Option<(String, &str)> {
    let end = s.find(|c: char| !is_ident(c)).unwrap_or(s.len());
    (end > 0).then(|| (s[..end].to_string(), &s[end..]))
}

/// The dotted tails a path fragment names — one, or every leaf of a `use` brace group.
fn expand(s: &str) -> Vec<String> {
    let s = s.trim_start();
    if !s.starts_with('{') {
        let mut out = String::new();
        let mut rest = s;
        while let Some((seg, after)) = ident(rest) {
            if !out.is_empty() {
                out.push_str("::");
            }
            out.push_str(&seg);
            if !after.starts_with("::") {
                break;
            }
            let tail = &after[2..];
            // `interact::{WorldClick, WorldRightClick}` — a group hanging off a path we have
            // already walked. Without this the walk stops dead at the brace and the whole group
            // collapses to the bare module path, which `drop_module_paths` then discards: every
            // item imported the ordinary Rust way was invisible to both walls. The brace arm
            // below only ever saw a group that began the path, which in real code it never does.
            if tail.trim_start().starts_with('{') {
                let head = out;
                return expand(tail)
                    .into_iter()
                    .map(|leaf| format!("{head}::{leaf}"))
                    .collect();
            }
            rest = tail;
        }
        return if out.is_empty() { vec![] } else { vec![out] };
    }
    let mut depth = 0usize;
    let mut end = s.len();
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    let mut depth = 0usize;
    for part in split_top(&s[1..end], &mut depth) {
        let part = part.trim();
        let part = part.split(" as ").next().unwrap_or(part).trim();
        if part.is_empty() || part == "self" {
            continue;
        }
        match part.find('{') {
            Some(b) => {
                let head = part[..b].trim_end_matches(':');
                for leaf in expand(&part[b..]) {
                    out.push(format!("{head}::{leaf}"));
                }
            }
            None => out.extend(expand(part)),
        }
    }
    out
}

/// Split on commas that are not inside a nested brace group.
fn split_top<'a>(s: &'a str, depth: &mut usize) -> Vec<&'a str> {
    let (mut out, mut start) = (Vec::new(), 0);
    for (i, c) in s.char_indices() {
        match c {
            '{' => *depth += 1,
            '}' => *depth = depth.saturating_sub(1),
            ',' if *depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// A path folded to the symbol it names: everything up to and including the first capitalised
/// segment (a type), or the whole path when every segment is lowercase (a function).
fn canon(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split("::") {
        out.push(seg);
        if seg.starts_with(char::is_uppercase) {
            break;
        }
    }
    out.join("::")
}

fn rs_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn render(items: &[(usize, &String)]) -> String {
    let mut v = items.to_vec();
    v.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    v.iter()
        .map(|(n, k)| format!("  {n:3} file(s)  {k}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A `use` group that spans lines counts every member — the blind spot 2338 closed. The
/// comment inside the group is cut, and the closing line's `};` ends the statement.
#[test]
fn a_use_group_spanning_lines_counts_every_member() {
    let text =
        "use benilla_world::interact::{\n    PickParts,\n    ray_mesh_bounds, // a lead\n    \
                ray_posed_mesh,\n};\nfn f() {}\n";
    let mut found: Vec<String> = paths_in(text, true, true)
        .into_iter()
        .map(|(p, _)| p)
        .collect();
    found.sort();
    assert_eq!(
        found,
        [
            "interact::PickParts",
            "interact::ray_mesh_bounds",
            "interact::ray_posed_mesh"
        ]
    );
}
