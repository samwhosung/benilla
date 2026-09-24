//! [`WorldCensus`]: what the engine drew this frame, as plain data for instruments. An instrument
//! calls [`WorldCensus::take`] on the frame it wants and formats the report itself; the line
//! shapes (`VIS_CENSUS`, `MAT_CHURN`, `ASSET_DUMP`) belong to the probes. The churn window is
//! opt-in, since [`WorldCensus::churn_counters`] costs a `MessageReader` per asset type per frame.

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

/// A read-only view of the engine's drawn state as one system parameter, so an instrument at
/// Bevy's 16-parameter ceiling can take all of it from the same frame.
#[derive(SystemParam)]
pub struct WorldCensus<'w, 's> {
    parts: Query<'w, 's, CensusData>,
    emitters: Query<'w, 's, &'static ParticleEmitter>,
    placements: Option<Res<'w, crate::terrain_stream::Placements>>,
    streamer: Option<Res<'w, crate::terrain_stream::TerrainStreamer>>,
    claim: Option<Res<'w, CameraInteriorClaim>>,
    windows: Option<Res<'w, ExteriorWindows>>,
    verdict: Option<Res<'w, ExteriorCullVerdict>>,
    cull_probe: Option<Res<'w, crate::wmo_portal::WmoCullProbe>>,
    skybox: Option<Res<'w, crate::skybox::CameraSkybox>>,
    ribbons: Option<Res<'w, crate::ribbons::RibbonVerdict>>,
    mats: Res<'w, Assets<WowModelMaterial>>,
    meshes: Res<'w, Assets<Mesh>>,
    images: Res<'w, Assets<bevy::image::Image>>,
    models: Res<'w, Assets<benilla_assets::M2Model>>,
    uv_reg: Res<'w, UvAnimMaterials>,
    tint_reg: Res<'w, TintAnimMaterials>,
    server: Res<'w, AssetServer>,
    churn: Option<ResMut<'w, ChurnCensus>>,
}

/// One frame's census as plain data, in the order an instrument prints it.
#[derive(Default)]
pub struct CensusReport {
    pub submeshes: usize,
    /// Placed doodad and WMO parts that no registered placement owns: zero in a healthy world, one
    /// per part of a doubled prop.
    pub orphan_parts: usize,
    /// Parts whose `VisibilityClass` lists a class twice; each is queued and drawn once per entry.
    pub stacked_parts: usize,
    /// The orphans by `(placement id, model label, parts)`, most parts first.
    pub orphans: Vec<(u32, String, usize)>,
    /// Resident tiles `(furnished, in window)`, off the streamer.
    pub tiles: Option<(usize, usize)>,
    /// Submeshes the render world will draw (`ViewVisibility`).
    pub drawn: usize,
    /// Visible submeshes per model subsystem, `(column, visible, of those gated)`, in fixed order.
    pub kinds: [(&'static str, usize, usize); 4],
    /// Resident submeshes per model subsystem, hidden or not, in `kinds`' column order.
    pub resident: [usize; 4],
    /// Resident submeshes per `EntityPathWhy` label (the streamer's reason a batch is an
    /// entity), most first; `"-"` for the untagged (units, GameObjects, the app lane).
    pub why: Vec<(&'static str, usize)>,
    /// Of the [`ExteriorScene`]-tagged submeshes: how many exist, are `Hidden`, are exempt (the
    /// camera's own placement), and have no `Aabb`, which the cull admits unconditionally.
    pub tagged: usize,
    pub hidden: usize,
    pub exempt: usize,
    pub no_aabb: usize,
    /// Tagged, bounded, not exempt and still not `Hidden`, as `(label, is a billboard card,
    /// count)`, most first: each one is a cull defect.
    pub escaped: Vec<(String, bool, usize)>,
    /// Visible submeshes per label, `(label, gated, count)`, ungated first, then most drawn.
    pub labels: Vec<(String, bool, usize)>,
    pub emitters: usize,
    pub active_emitters: usize,
    pub particles: usize,
    /// Ribbon trails alive and how many wrote a strip; `None` without the ribbon lane.
    pub ribbons: Option<(usize, usize)>,
    /// The WMO group the camera claims (`"g07"`, or `"none"` outdoors); `None` without portals.
    pub room: Option<String>,
    /// `"unrestricted"`, or the number of window sub-frusta. `None` with [`CensusReport::room`].
    pub windows: Option<String>,
    /// The eye the portal authority computed [`Self::room`], [`Self::windows`] and the PVS from,
    /// the last propagated camera transform; after a teleport it lags the drawn eye.
    pub pvs_eye: Option<Vec3>,
    /// The backdrop: `"dome"` for the `Light.dbc` gradient, else the WMO skybox model (whose
    /// batches carry no [`ModelPart`], so no other count sees them); `None` without that lane.
    pub sky: Option<String>,
    /// What the exterior cull did this frame, `None` if it did not run.
    pub cull: Option<CullTerms>,
    /// Resident asset counts: the leak meter.
    pub mats: usize,
    /// Built materials `model_render::lazy` parks until something visible binds them.
    pub mats_parked: usize,
    pub meshes: usize,
    pub images: usize,
    pub uv_anims: usize,
    pub tint_anims: usize,
    /// `AssetEvent::Modified` totals per asset type since the window opened, sorted by label.
    /// Empty unless [`WorldCensus::churn_counters`] is installed.
    pub churn: Vec<(&'static str, usize)>,
}

/// [`ExteriorCullVerdict`] as plain numbers; that type documents each term.
pub struct CullTerms {
    /// The window count it was given, or `"unrestricted"`.
    pub windows: String,
    pub frusta: usize,
    pub tested: usize,
    pub hidden: usize,
    pub unbounded: usize,
    /// The body leg, counted apart from `tested` and `hidden`.
    pub bodies: usize,
    pub bodies_hidden: usize,
    /// The liquid subset of `tested` and `hidden`.
    pub liquid: usize,
    pub liquid_hidden: usize,
}

impl WorldCensus<'_, '_> {
    /// Installs the per-asset-type churn counters in `First`, before anything modifies an asset.
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

    /// Adds an asset type to the churn window under `label`, for a host's own materials; call it
    /// after `churn_counters`, which creates the tally.
    pub fn count_churn<A: bevy::asset::Asset>(app: &mut App, label: &'static str) {
        app.add_systems(First, churn_counter::<A>(label));
    }

    /// Opens a fresh churn window; call it on the first sampled frame so warm-up is not counted.
    pub fn restart_churn(&mut self) {
        if let Some(churn) = self.churn.as_mut() {
            churn.0.clear();
        }
    }

    /// Live particles now: a fold over the emitters, cheap enough to sample every frame.
    pub fn live_particles(&self) -> usize {
        self.emitters.iter().map(|p| p.live()).sum()
    }

    /// Snapshot this frame.
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
            stacked_parts += usize::from(class.is_some_and(|c| c.len() > 1));
            // A placed part outside every placement's list outlived a respawn, except the retained
            // pass's fader exiles (`static_gx::cull`), recorded on their fader seed instead.
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
                // The camera's own placement is supposed to draw, so it is not an escapee.
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

    /// Every resident image, mesh and model by asset path, sorted, plus the unpathed
    /// (runtime-built) counts in that order; apart from [`Self::take`] because it is expensive.
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

/// The census column order: entry `i` names [`kind_index`]'s slot `i`. Other tools diff these
/// columns, so the order is fixed.
const KIND_COLUMNS: [&str; 4] = ["doodad", "wmo", "creature", "gameobject"];

/// The census's own index for a [`ModelKind`], pinned to [`KIND_COLUMNS`].
fn kind_index(kind: ModelKind) -> usize {
    match kind {
        ModelKind::Doodad => 0,
        ModelKind::Wmo => 1,
        ModelKind::Creature => 2,
        ModelKind::GameObject => 3,
    }
}

/// `AssetEvent::Modified` counts per asset type across a window: a modified material rebuilds its
/// uniform buffers and bind group that frame (non-bindless), a modified image or mesh re-uploads.
#[derive(Resource, Default)]
struct ChurnCensus(std::collections::BTreeMap<&'static str, usize>);

/// Counts asset type `A`'s `Modified` events under `label`, a short stable name (an
/// `ExtendedMaterial` alias's `type_name` is unreadable).
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
