//! **[`WorldCensus`] — what the engine drew this frame, published as a fact.**
//!
//! Every number an instrument wants about a rendered frame — how many submeshes exist, how many
//! survived to `ViewVisibility`, which subsystem each belongs to, what the exterior-scene gate did,
//! how many emitters are alive, what the asset caches are holding, what got rewritten since the
//! last frame — is a fact the *renderer* owns. Before this module the probes read it by querying
//! the renderer's own components and resources directly, and that one file
//! (`capture/probes/live_fps.rs`) named nine engine internals nobody else outside the engine
//! needed: four material aliases, the two exterior-cull types, the two portal types, and the
//! camera's interior claim. Decision 1164 sorts all nine into CLOSE with the same remedy —
//! *publish the census, not the components* — and this is that census.
//!
//! It is one read-only [`SystemParam`] and one snapshot type. An instrument adds the param, calls
//! [`WorldCensus::take`] on the frame it cares about, and formats the snapshot however its own
//! output contract wants. **No `println!` lives here**: the line shapes (`VIS_CENSUS`,
//! `MAT_CHURN`, `ASSET_DUMP`) are the probes' greppable contract, not the engine's, and an engine
//! that formats its callers' output has the boundary backwards.
//!
//! The one part that is opt-in is the churn window: counting `AssetEvent::Modified` per asset type
//! costs a `MessageReader` per type per frame, so [`WorldCensus::churn_counters`] installs them
//! and an ordinary run carries none. **Which** types are counted is engine knowledge (it is the
//! engine's material set); **whether** to count is the instrument's call.

use std::collections::HashMap;

use bevy::camera::primitives::Aabb;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::doodad_anim::{TintAnimMaterials, UvAnimMaterials};
use crate::exterior_cull::{ExteriorCullVerdict, ExteriorScene};
use crate::interact::WorldObject;
use crate::model_render::{ModelKind, ModelPart};
use crate::particles::ParticleEmitter;
use crate::wmo_portal::{CameraInteriorClaim, ExteriorWindows, WmoGroupVis};
use benilla_assets::materials::WowModelMaterial;

/// A read-only view of the engine's drawn state — the whole of it, as one system parameter.
///
/// Deliberately one param and not eight: `drive_live_fps` sits at Bevy's 16-param ceiling, and
/// every instrument that wants any of this wants most of it, taken from the *same frame*. Two
/// instruments each assembling their own half is how the two disagree.
#[derive(SystemParam)]
pub struct WorldCensus<'w, 's> {
    /// Every spawned model submesh, with the facts that make a visible one accountable.
    parts: Query<'w, 's, CensusData>,
    emitters: Query<'w, 's, &'static ParticleEmitter>,
    /// The placement registry, for the duplicate census (`orphan_parts=`): a placed part alive
    /// that no registered placement owns. Optional for a viewer with no streamer.
    placements: Option<Res<'w, crate::terrain_stream::Placements>>,
    streamer: Option<Res<'w, crate::terrain_stream::TerrainStreamer>>,
    /// The exterior-scene gate's two terms and what the cull actually did with
    /// them. Optional so the census works in a viewer that has not installed the portal system.
    claim: Option<Res<'w, CameraInteriorClaim>>,
    windows: Option<Res<'w, ExteriorWindows>>,
    verdict: Option<Res<'w, ExteriorCullVerdict>>,
    /// The visibility authority's own pose (see [`CensusReport::pvs_eye`]).
    cull_probe: Option<Res<'w, crate::wmo_portal::WmoCullProbe>>,
    /// Which backdrop is drawing — the gradient dome, or a building's own MOSB sky
    /// ([`crate::skybox`]). Optional for the same reason as the portal terms above.
    skybox: Option<Res<'w, crate::skybox::CameraSkybox>>,
    /// The ribbon lane's own verdict — the third effect population, counted nowhere else.
    ribbons: Option<Res<'w, crate::ribbons::RibbonVerdict>>,
    mats: Res<'w, Assets<WowModelMaterial>>,
    meshes: Res<'w, Assets<Mesh>>,
    images: Res<'w, Assets<bevy::image::Image>>,
    models: Res<'w, Assets<benilla_assets::M2Model>>,
    uv_reg: Res<'w, UvAnimMaterials>,
    tint_reg: Res<'w, TintAnimMaterials>,
    server: Res<'w, AssetServer>,
    /// Present only once [`WorldCensus::churn_counters`] has been installed.
    churn: Option<ResMut<'w, ChurnCensus>>,
}

/// One frame's census, in the order an instrument prints it. Plain data on purpose: an instrument
/// holding this holds no engine component, no engine resource and no engine type it has to name.
#[derive(Default)]
pub struct CensusReport {
    /// Every model submesh that exists.
    pub submeshes: usize,
    /// Placed (doodad/WMO) parts alive that no registered placement owns — the duplicate
    /// census. Zero in a healthy world; a doubled prop is exactly one of these per part.
    pub orphan_parts: usize,
    /// Parts whose `VisibilityClass` holds a duplicate entry — each is queued and drawn once
    /// per entry (`model_render::park`'s dedup is the fix; this is the census that names a
    /// regression).
    pub stacked_parts: usize,
    /// The orphans by `(placement id, model label, parts)`, most parts first.
    pub orphans: Vec<(u32, String, usize)>,
    /// Resident tiles `(furnished, in window)`, off the streamer.
    pub tiles: Option<(usize, usize)>,
    /// …and how many of them the render world will actually draw (`ViewVisibility`).
    pub drawn: usize,
    /// Visible submeshes per model subsystem — `(column name, visible, of-those-gated)` — in a
    /// fixed column order, because a census whose columns move cannot be diffed across runs.
    pub kinds: [(&'static str, usize, usize); 4],
    /// Resident submeshes per model subsystem, same column order as `kinds` — hidden or not.
    /// A city's hidden twins (the retained pass draws them) ride every per-`Mesh3d` sweep
    /// whether they draw or not, and this is the count that prices them.
    pub resident: [usize; 4],
    /// Resident submeshes per `EntityPathWhy` label (the streamer's reason a batch is an
    /// entity), most first; `"-"` for the untagged (units, GameObjects, the app lane).
    pub why: Vec<(&'static str, usize)>,
    /// Of every [`ExteriorScene`]-tagged submesh: how many exist, how many the cull wrote `Hidden`
    /// on, how many were exempt (the camera's own placement), and how many carry
    /// **no `Aabb`**, which is the cull's fail-open arm admitting them unconditionally.
    pub tagged: usize,
    pub hidden: usize,
    pub exempt: usize,
    pub no_aabb: usize,
    /// Tagged, bounded, not exempt, and yet not `Hidden` — the escapees, `(label, is a billboard
    /// card, count)`, most first. A tagged object the cull left un-hidden is a defect by
    /// construction, which is why this is not behind a flag.
    pub escaped: Vec<(String, bool, usize)>,
    /// Visible submeshes per distinct label, `(label, gated, count)` — ungated first (the leak
    /// candidates), then most-drawn first. The "so WHICH trees are they?" list.
    pub labels: Vec<(String, bool, usize)>,
    pub emitters: usize,
    pub active_emitters: usize,
    pub particles: usize,
    /// Ribbon trails alive, and how many wrote a strip — `None` if the lane is not installed.
    /// The `emitters`/`particles` pair above covers only the QUAD half of the effect stream;
    /// trails are the other half and had no counter at all.
    pub ribbons: Option<(usize, usize)>,
    /// The WMO group the camera claims — `"g07"`, or `"none"` over open world. `None` when the
    /// portal system is not installed at all, which is a different statement from "not indoors".
    pub room: Option<String>,
    /// `"unrestricted"`, or the number of window sub-frusta. `None` with [`CensusReport::room`].
    pub windows: Option<String>,
    /// **The eye the portal authority computed [`Self::room`], [`Self::windows`] and every group's
    /// PVS from.** Not necessarily the eye this frame draws from: the authority runs in `Update`
    /// off the camera's propagated transform, so it is answering about wherever the camera was
    /// when that transform was last written. Walking makes the difference a centimetre; a
    /// teleport makes it the whole jump, and this is the column that says so.
    pub pvs_eye: Option<Vec3>,
    /// The backdrop actually drawing: `"dome"` for the `Light.dbc` gradient, else the WMO skybox
    /// model a building's PVS asked for. `None` when the WMO-sky lane is not installed.
    ///
    /// Here because a skybox is the one draw that can repaint the whole frame while being
    /// invisible to every other line in this report: its batches carry no [`ModelPart`], so they
    /// are not in `submeshes`, not in `drawn`, not in `kinds`, and not in `labels`. Standing
    /// outdoors in Tanaris with a purple sky and no purple *object* on any list is exactly the
    /// state that has to be readable, and before this it was not.
    pub sky: Option<String>,
    /// What the exterior cull did this frame, `None` if it did not run.
    pub cull: Option<CullTerms>,
    /// Resident asset counts — the leak meter. A tour probe reading the same counts as a fresh
    /// control is what "torn down" means, machine-checked.
    pub mats: usize,
    /// Built materials parked by `model_render::lazy`, not yet assets — the variants nothing
    /// visible has bound. `mats` counts realized ones only.
    pub mats_parked: usize,
    pub meshes: usize,
    pub images: usize,
    pub uv_anims: usize,
    pub tint_anims: usize,
    /// `AssetEvent::Modified` totals per asset type since the window opened, sorted by type name.
    /// Empty unless [`WorldCensus::churn_counters`] is installed.
    pub churn: Vec<(&'static str, usize)>,
}

/// [`ExteriorCullVerdict`] relayed as plain numbers — see that type for what each term means and
/// why `tested` is the one no other instrument can see.
pub struct CullTerms {
    /// The window count it was given, or `"unrestricted"`.
    pub windows: String,
    pub frusta: usize,
    pub tested: usize,
    pub hidden: usize,
    pub unbounded: usize,
    /// The body leg, kept apart from `tested`/`hidden` — see the verdict's own note for why summing
    /// a two-dozen audience into a tens-of-thousands one erases it.
    pub bodies: usize,
    pub bodies_hidden: usize,
    /// The liquid subset of `tested`/`hidden` — see the verdict's own note for why a few dozen
    /// surfaces summed into tens of thousands of terrain cells is an invisible leg (1652).
    pub liquid: usize,
    pub liquid_hidden: usize,
}

impl WorldCensus<'_, '_> {
    /// Install the per-asset-type churn counters (in `First`, before anything can modify an
    /// asset this frame). Off by default — see the module header for the split of who decides.
    pub fn churn_counters(app: &mut App) {
        app.init_resource::<ChurnCensus>();
        Self::count_churn::<bevy::image::Image>(app, "image");
        Self::count_churn::<Mesh>(app, "mesh");
        Self::count_churn::<StandardMaterial>(app, "std");
        Self::count_churn::<benilla_assets::materials::TerrainMaterial>(app, "terrain");
        Self::count_churn::<WowModelMaterial>(app, "model");
        Self::count_churn::<benilla_assets::materials::WdlMaterial>(app, "wdl");
        Self::count_churn::<benilla_assets::materials::LiquidMaterial>(app, "liquid");
        Self::count_churn::<crate::sky::SkyMaterial>(app, "sky");
        Self::count_churn::<crate::sun::CelestialMaterial>(app, "celestial");
        Self::count_churn::<crate::sun::StarMaterial>(app, "star");
        Self::count_churn::<crate::clouds::CloudMaterial>(app, "cloud");
    }

    /// Add one more asset type to the churn window under `label`. The engine counts its own
    /// materials in [`WorldCensus::churn_counters`]; a host with materials of its own (the UI
    /// pass, a tool's overlay) folds them in here rather than keeping a second, disagreeing
    /// census. Call after `churn_counters`, which is what creates the tally.
    pub fn count_churn<A: bevy::asset::Asset>(app: &mut App, label: &'static str) {
        app.add_systems(First, churn_counter::<A>(label));
    }

    /// Open a fresh churn window. Warmup noise — streaming, shader warms — otherwise reads as a
    /// steady-state ratchet, so the instrument calls this on the first frame it actually samples.
    pub fn restart_churn(&mut self) {
        if let Some(churn) = self.churn.as_mut() {
            churn.0.clear();
        }
    }

    /// Snapshot this frame.
    /// Live particles this instant — the one census number cheap enough to sample per frame
    /// (a fold over the emitters, no part walk): a spell burst's signature on a tail frame.
    pub fn live_particles(&self) -> usize {
        self.emitters.iter().map(|p| p.live()).sum()
    }

    pub fn take(&self) -> CensusReport {
        let own_instance = self
            .claim
            .as_ref()
            .and_then(|c| c.0)
            .map(|c| c.room.instance);

        let mut kinds = [(0usize, 0usize); 4];
        let mut resident = [0usize; 4];
        let mut why: HashMap<&'static str, usize> = HashMap::new();
        let mut labels: HashMap<(String, bool), usize> = HashMap::new();
        let mut escaped: HashMap<(String, bool), usize> = HashMap::new();
        let (mut tagged, mut hidden, mut exempt_n, mut no_aabb) = (0, 0, 0, 0);
        let (mut submeshes, mut drawn) = (0usize, 0usize);

        let owned = self.placements.as_ref().map(|p| p.owned());
        let mut orphans: HashMap<(u32, String), usize> = HashMap::new();
        let mut stacked_parts = 0usize;
        for (entity, vis, part, gated, object, want, aabb, card, group, path_why, class) in
            self.parts.iter()
        {
            submeshes += 1;
            // A part whose `VisibilityClass` lists its mesh class more than once is queued
            // that many times a frame — drawn stacked on itself (`model_render::park`).
            stacked_parts += usize::from(class.is_some_and(|c| c.len() > 1));
            // The duplicate census: a doodad/WMO part is spawned by exactly one placement and
            // recorded on it; one alive outside every placement's list outlived a respawn. The
            // one population that lives outside the registry by design is the retained pass's
            // fader EXILES (`static_gx::cull`): the feather-band respawns are
            // recorded on their fader seed, not on the placement, and they name themselves.
            let exile = path_why.is_some_and(|w| w.0 == "exile");
            if let (Some(owned), Some(o), false) = (owned.as_ref(), object, exile) {
                if matches!(o.kind, ModelKind::Doodad | ModelKind::Wmo) && !owned.contains(&entity)
                {
                    *orphans.entry((o.id, o.label.clone())).or_default() += 1;
                }
            }
            resident[kind_index(part.kind)] += 1;
            *why.entry(path_why.map_or("-", |w| w.0)).or_default() += 1;
            drawn += usize::from(vis.get());
            if gated {
                // The camera's own placement is not exterior scene to itself and
                // is *supposed* to draw; without that subtraction the escapee list is all
                // room-you-are-in furniture and says nothing.
                let exempt = group.is_some_and(|g| Some(g.instance) == own_instance);
                tagged += 1;
                hidden += usize::from(*want == Visibility::Hidden);
                no_aabb += usize::from(aabb.is_none());
                exempt_n += usize::from(exempt);
                if *want != Visibility::Hidden && aabb.is_some() && !exempt {
                    let label = object.map_or("<unlabelled>", |o| o.label.as_str());
                    *escaped.entry((label.to_string(), card)).or_default() += 1;
                }
            }
            if !vis.get() {
                continue;
            }
            let slot = &mut kinds[kind_index(part.kind)];
            slot.0 += 1;
            slot.1 += usize::from(gated);
            if let Some(o) = object {
                *labels.entry((o.label.clone(), gated)).or_default() += 1;
            }
        }

        let orphan_parts = orphans.values().sum();
        let mut orphans: Vec<_> = orphans.into_iter().map(|((i, l), n)| (i, l, n)).collect();
        orphans.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
        let mut escaped: Vec<_> = escaped.into_iter().map(|((l, c), n)| (l, c, n)).collect();
        escaped.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
        let mut labels: Vec<_> = labels.into_iter().map(|((l, g), n)| (l, g, n)).collect();
        labels.sort_by(|a, b| a.1.cmp(&b.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));

        let (emitters, active_emitters, particles) = self
            .emitters
            .iter()
            .fold((0usize, 0usize, 0usize), |(e, a, l), p| {
                (e + 1, a + usize::from(p.live() > 0), l + p.live())
            });

        let (room, windows) = match self.windows.as_deref() {
            Some(w) => (
                Some(match self.claim.as_ref().and_then(|c| c.0) {
                    Some(claim) => format!("g{:02}", claim.room.group),
                    None => "none".to_string(),
                }),
                Some(match w {
                    ExteriorWindows::Unrestricted => "unrestricted".to_string(),
                    ExteriorWindows::Windows(rects) => rects.len().to_string(),
                }),
            ),
            None => (None, None),
        };

        let sky = self.skybox.as_ref().map(|s| {
            s.0.as_deref()
                .map_or_else(|| "dome".to_string(), str::to_ascii_lowercase)
        });

        CensusReport {
            submeshes,
            drawn,
            stacked_parts,
            orphan_parts,
            orphans,
            tiles: self.streamer.as_ref().map(|s| s.residency()),
            why: {
                let mut v: Vec<_> = why.into_iter().collect();
                v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
                v
            },
            resident,
            kinds: std::array::from_fn(|i| (KIND_COLUMNS[i], kinds[i].0, kinds[i].1)),
            tagged,
            hidden,
            exempt: exempt_n,
            no_aabb,
            escaped,
            labels,
            emitters,
            active_emitters,
            particles,
            room,
            windows,
            pvs_eye: self.cull_probe.as_deref().map(|p| p.eye),
            sky,
            ribbons: self.ribbons.as_deref().map(|r| (r.trails, r.drawn)),
            cull: self.verdict.as_deref().map(|v| CullTerms {
                windows: v
                    .windows
                    .map_or("unrestricted".to_string(), |n| n.to_string()),
                frusta: v.frusta,
                tested: v.tested,
                hidden: v.hidden,
                unbounded: v.unbounded,
                bodies: v.bodies,
                bodies_hidden: v.bodies_hidden,
                liquid: v.liquid,
                liquid_hidden: v.liquid_hidden,
            }),
            mats: self.mats.len(),
            mats_parked: crate::model_render::lazy::pending_len(),
            meshes: self.meshes.len(),
            images: self.images.len(),
            uv_anims: self.uv_reg.0.len(),
            tint_anims: self.tint_reg.0.len(),
            churn: self
                .churn
                .as_ref()
                .map(|c| c.0.iter().map(|(k, n)| (*k, *n)).collect())
                .unwrap_or_default(),
        }
    }

    /// Every resident image/mesh/model by asset-server path, sorted, plus the counts that have no
    /// path (runtime-built) in that same order. Diffing a tour probe's inventory against a fresh
    /// control's names exactly which files a teardown left behind — the leak meter's magnifying
    /// glass, and the one census answer expensive enough to ask for separately.
    pub fn resident_assets(&self) -> (Vec<String>, [usize; 3]) {
        let mut lines: Vec<String> = Vec::new();
        let mut unpathed = [0usize; 3];
        let kinds: [(&str, Vec<bevy::asset::UntypedAssetId>); 3] = [
            (
                "image",
                self.images.ids().map(|i| i.untyped()).collect::<Vec<_>>(),
            ),
            (
                "mesh",
                self.meshes.ids().map(|i| i.untyped()).collect::<Vec<_>>(),
            ),
            (
                "model",
                self.models.ids().map(|i| i.untyped()).collect::<Vec<_>>(),
            ),
        ];
        for (slot, (kind, ids)) in kinds.into_iter().enumerate() {
            for id in ids {
                match self.server.get_path(id) {
                    Some(p) => lines.push(format!("{kind} {p}")),
                    None => unpathed[slot] += 1,
                }
            }
        }
        lines.sort();
        (lines, unpathed)
    }
}

/// What the census reads off every model submesh — the query shape.
type CensusData = (
    Entity,
    &'static ViewVisibility,
    &'static ModelPart,
    Has<ExteriorScene>,
    Option<&'static WorldObject>,
    &'static Visibility,
    Option<&'static Aabb>,
    Has<crate::billboard::BillboardCard>,
    Option<&'static WmoGroupVis>,
    Option<&'static crate::model_render::EntityPathWhy>,
    Option<&'static bevy::camera::visibility::VisibilityClass>,
);

/// The census column order, pinned: entry `i` names [`kind_index`]'s slot `i`. Column positions
/// in a line other tools diff, so they move for nobody.
const KIND_COLUMNS: [&str; 4] = ["doodad", "wmo", "creature", "gameobject"];

/// [`ModelKind`] has a private index; the census pins its own — see [`KIND_COLUMNS`].
fn kind_index(kind: ModelKind) -> usize {
    match kind {
        ModelKind::Doodad => 0,
        ModelKind::Wmo => 1,
        ModelKind::Creature => 2,
        ModelKind::GameObject => 3,
    }
}

/// `AssetEvent::Modified` counts per asset type across a window. A modified material re-creates
/// its uniform buffers + bind group that frame (the Metal non-bindless path), and a modified
/// image/mesh re-uploads — the teleport leak's CPU engine was exactly a per-frame ratchet of this
/// shape, so the floor hunt names the types instead of guessing suspects one at a time.
#[derive(Resource, Default)]
struct ChurnCensus(std::collections::BTreeMap<&'static str, usize>);

/// One census counter for asset type `A`, folding this frame's `Modified` events in under `label`
/// (a short stable name — the `type_name` of an `ExtendedMaterial` alias is unreadable).
fn churn_counter<A: bevy::asset::Asset>(
    label: &'static str,
) -> impl FnMut(MessageReader<bevy::asset::AssetEvent<A>>, ResMut<ChurnCensus>) {
    move |mut reader, mut census| {
        let n = reader
            .read()
            .filter(|e| matches!(e, bevy::asset::AssetEvent::Modified { .. }))
            .count();
        if n > 0 {
            *census.0.entry(label).or_default() += n;
        }
    }
}
