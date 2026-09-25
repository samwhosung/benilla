//! The world API wall: every `benilla-world` item that `benilla-app` code names, counted from
//! source. [`PUBLISHED`] is the designed API, [`SORTED_LEAKS`] the crossings filed to close and
//! how, and anything else that crosses is an unsorted leak. Only the leak count is ratcheted,
//! toward zero; a table row nothing names any more fails the test until it is deleted.
//!
//! Not counted: `#[cfg(test)]` bodies (a test naming an internal is not an API consumer) and
//! comments. Plugin registrations in the composition root count like anything else.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The engine's module set when the world lives inside `benilla-app`: an engine module missing
/// here hides its leaks. Unused once `crates/benilla-world` exists, which the scan checks.
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

/// The instruments inside the engine module set: they sit above the engine, so they are not part
/// of the doorway being measured.
const INSTRUMENT_ROOTS: &[&str] = &["art_scope", "debug_panel", "perf", "pipe_warm"];

/// The `benilla-app` instruments (probes, the crash hook, the dev panels) that consume
/// `benilla-world` like any caller. An API shaped by what a debugger wanted to poke is not the
/// designed API, so what only they name is counted separately, outside the gate.
const INSTRUMENT_CONSUMERS: &[&str] = &["capture", "crash", "debug_panel", "perf", "pipe_warm"];

/// Is this file one of the app-side instruments?
fn is_instrument_consumer(rel: &str) -> bool {
    let root = rel.split(['/', '.']).next().unwrap_or("");
    INSTRUMENT_CONSUMERS.contains(&root)
}

/// The designed API: every engine item game code may name, with the record that published it
/// (`wall`: published without one). Adding a row is a claim made in review, with its record.
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

/// The sorted leaks: crossings filed to close, with how. `CLOSE/absorb`: a facade takes the call;
/// `CLOSE/invert`: the engine publishes the fact instead of the game reaching in;
/// `CLOSE/move-engine`: engine code in a game file moves engine-side; `CLOSE/move-game`: gameplay
/// moves out of the engine. Closing a leak is deleting its row.
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

/// The gate on the leak count. Lower it when a leak closes.
const LEAK_CEILING: usize = 107;

/// How far under [`LEAK_CEILING`] the count may sit before the test asks for the ceiling to be
/// lowered: one closure does not fail the gate, a whole stage of work cannot go unrecorded.
const LEAK_SLACK: usize = 10;

#[test]
fn the_world_api_doorway_stays_shut() {
    let all = measure();
    // An item only an instrument names is outside the doorway (`INSTRUMENT_CONSUMERS`); one a
    // game module also names is inside it.
    let (probes, surface): (BTreeMap<_, _>, BTreeMap<_, _>) = all
        .into_iter()
        .partition(|(_, files)| files.iter().all(|f| is_instrument_consumer(f)));
    // The doorway is the published API plus the leaks; only the leaks gate.
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
    // Always print the numbers (`--nocapture` shows them).
    eprintln!(
        "world API surface: {} items — {n} leaks (ceiling {LEAK_CEILING}, slack {LEAK_SLACK}; \
         {sorted} sorted by 1164, {} unsorted), {} published; + {} named only by the instruments",
        published.len() + n,
        n - sorted,
        published.len(),
        probes.len()
    );

    // A row no game file names any more must go: a stale published row would let the item cross
    // again for free.
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

    // `WOW_API_DUMP=1` prints the surface itself, most-named first, each item with its table tag.
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

/// The wall that points the other way: an engine file naming a gameplay item is a dependency
/// `benilla-world` cannot express. Zero, and the crate graph keeps it there.
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
        // Without the world crate the file's module decides its side; with it, every file here
        // is on the game side.
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

/// Drop every entry that is only the path to another entry: `particles::buffer` and
/// `mesh_tag::spawn_tag` look alike, but a module path is a strict prefix of another named item
/// and a function is a prefix of nothing.
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

/// Every gameplay item named by an engine file. Empty once the world crate exists, where such a
/// name is a compile error.
fn measure_back() -> BTreeMap<String, BTreeSet<String>> {
    let app = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if app
        .parent()
        .is_some_and(|c| c.join("benilla-world").is_dir())
    {
        return BTreeMap::new(); // the crate graph enforces it
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
        // Instruments may see both sides: they sit above the engine.
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

/// Every source file that is a test module in its own file: declared `#[cfg(test)] mod x;`,
/// with its body in `x.rs` or `x/mod.rs` beside its parent. Its names do not count.
fn test_module_files(src: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for file in rs_files(src) {
        let text = std::fs::read_to_string(&file).unwrap();
        // `a.rs`'s `mod b;` is `a/b.rs`; so is `a/mod.rs`'s.
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
/// It follows `crate::root::` (or `benilla_world::` with the crate), expands a `use` brace group
/// into its items, and stops a path at its first capitalised segment (`WorldAssets::get` is one).
fn paths_in(text: &str, split: bool, want_engine: bool) -> Vec<(String, usize)> {
    let prefix = if split { "benilla_world::" } else { "crate::" };
    let mut found = Vec::new();
    let mut depth: i32 = 0;
    // A `#[cfg(test)]` guards the item after it. `test_at` arms at the attribute's depth `d`: a
    // braced item ends when the depth comes back to `d` (it never drops below), an unbraced one
    // (`mod tests;`, `use …;`) at its semicolon.
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
                    // A crate-root item (`crate::SPAWN_XY`) belongs to the game, since `lib.rs`
                    // stays in `benilla-app`; an engine file naming one is a reverse crossing.
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
                    // An instrument is neither side: outside the doorway, and not a reverse
                    // dependency either, since `WorldPlugins` registers it.
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
                            // A bare module name is not an item; `drop_module_paths` misses one
                            // whose members are named only where the scan does not look.
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

/// The file's lines, with a `use` brace group that spans several lines joined into one, since the
/// scan is line-based. A joined line keeps its first line's number; a `//` inside it is cut (a
/// `use` has no string literal for one to sit in).
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

/// The dotted tails a path fragment names: one, or every leaf of a `use` brace group.
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
            // `interact::{WorldClick, WorldRightClick}`: a group hanging off a walked path.
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

/// A `use` group that spans lines counts every member; the comment inside it is cut, and the
/// closing `};` ends the statement.
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
