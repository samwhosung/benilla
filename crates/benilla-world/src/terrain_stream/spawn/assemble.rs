//! The placed-model assembler: [`spawn_model_entities`] builds a model's submesh entities with the
//! full doodad/WMO component set (production materials, fade, mesh tags, the doodad anim host and
//! its skinned twins). Two consumers — the terrain streamer's world-static placements
//! ([`super::spawn_loaded_placements`]) and the WMO-display gameobject's doodad props
//! (`crate::entities`' `wmo_props`).

use std::sync::Arc;

use benilla_assets::{M2Model, ModelSubmesh};
use benilla_formats::ModelBlend;
use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::NoAutoAabb;
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use bevy::render::render_resource::Buffer;

use crate::billboard::BillboardCard;
use crate::doodad_anim::DoodadAnimHost;
use crate::mesh_tag::alpha_bits;
use crate::model_fade::DoodadFade;
use crate::model_render::{model_material, MaterialCache, ShadeSel};
use crate::model_render::{ModelKind, ModelPart};
use benilla_assets::materials::WowModelMaterial;

/// What one placement's animation host armed, for the consumers that spawn alongside its submeshes.
/// All fields are *per placement*, not per model: the anchors are this instance's, and `arm` is
/// this instance's own anim root, whose live player names the sequence it is currently playing.
pub struct PlacementHost {
    /// `(bone, anchor)` — the consumer anchors this placement minted (decision 1365): one entity
    /// per bone something actually rides, registered in the host's `RigPose` (its compose passes
    /// re-seat them) and pre-minted here for every emitter and ribbon bone the model authors, so
    /// the fx spawns that run after the pose buffer is attached only look entities up.
    anchors: Vec<(u16, Entity)>,
    /// This placement's anim-root entity. It rides the returned entity list so it despawns with the
    /// placement, but it is **not geometry** — a caller tagging "everything this placement draws"
    /// has to be able to leave it out, or it lands in a cull that can only fail open on a boundless
    /// entity (decision 0784). Distinct from `arm`, which is `None` unless a sequence was armed.
    pub(crate) root: Entity,
    /// The anim-root entity IF this placement armed a sequence; `None` on the gseq-only tier, where
    /// nothing was armed and slot 0 is the honest answer.
    ///
    /// This used to be the armed slot as a plain `usize`, captured once at spawn. That was right
    /// only while a placed doodad kept one variation for life — it re-rolls every play-window
    /// (decision 0768), so a captured slot goes stale within about a second and the consumer has to
    /// resolve against the live player instead.
    pub arm: Option<Entity>,
}

impl PlacementHost {
    /// The anchor standing in for `bone` — what `joints.get(bone)` used to answer. `None` = the
    /// bone hosts nothing this placement consumes (or is outside the skeleton): the consumer
    /// falls back exactly as it did on a missing joint.
    pub fn anchor(&self, bone: u16) -> Option<Entity> {
        self.anchors
            .iter()
            .find(|&&(b, _)| b == bone)
            .map(|&(_, e)| e)
    }
}

/// What [`spawn_model_entities`] spawned.
pub struct SpawnedModel {
    /// The caller's despawn list — everything this placement owns as world roots (anim host,
    /// parts, world-root cards; owner-following cards manage their own lifecycle and stay out).
    pub entities: Vec<Entity>,
    /// Index-parallel with the `submeshes` slice: the entity each batch spawned, `None` where
    /// no entity exists (a missing render form; historically also the retired `WOW_MEGA_STATIC`
    /// bracket's divert). This exists
    /// because POSITION IS A CONTRACT: the WMO path zips per-batch group indices against what
    /// was spawned, and a skipped batch must hold its slot rather than shift every later batch
    /// onto the wrong group — which is exactly what the bracket's bare `continue` did to the
    /// surviving entities of a diverted building (found by 1417's constraint map).
    pub by_batch: Vec<Option<Entity>>,
    /// The anim host, when the model animates (see [`PlacementHost`]).
    pub host: Option<PlacementHost>,
}

/// Spawn a model's submeshes at `transform` with the full doodad/WMO component set. Returns the spawned
/// entities (for refcounted despawn).
///
/// `pub(crate)`: two consumers — the terrain streamer's world-static placements here, and the
/// WMO-display GameObject's doodad props ([`crate::entities`]'s `wmo_props`, the ship's sails), which
/// pass a doodad-LOCAL `transform` + `card_owner` and parent the returned entities under the moving
/// gameobject (every downstream system reads propagated `GlobalTransform`s, so the composition holds).
pub fn spawn_model_entities(
    commands: &mut Commands,
    mat_cache: &mut MaterialCache,
    materials: &mut Assets<WowModelMaterial>,
    light: &Buffer,
    submeshes: &[ModelSubmesh],
    // The model's app-built render forms (decision 0834), index-parallel with `submeshes`: the
    // static handle + its build-time `Aabb` per batch, and the skinned twins when this model's
    // lane rigs (`None` otherwise). Callers gate on `ModelForms::require`, so by the time a
    // placement spawns these are complete.
    forms: crate::model_forms::FormSlices<'_>,
    transform: Transform,
    // This placement's dev-instrument identity — model path, placement uniqueId, kind — shared
    // by every batch it spawns. It is an INPUT rather than the blanket tag it used to be
    // (`tag_world_object`, still applied to the returned entities) because a batch that DIVERTS
    // spawns no entity to tag: the consolidating lanes carry this Arc into their own pick
    // members instead, which is what keeps a merged or retained placement nameable (1534).
    // `kind` also stands in for the old `is_wmo` flag — one source, so they cannot disagree.
    object: &Arc<crate::interact::WorldObject>,
    // The static terrain-shade selector for every batch (the MCSH sample at this placement's base →
    // `ShadeSel::Lit`/`Matte`/`Shaded`; see `model_render::ShadeSel`). Ignored by WMO group geometry
    // and interior props (their lighting lanes don't read it).
    shade: ShadeSel,
    // `Some(slot)` for an INTERIOR M2 prop: its folded SH probe's table slot, carried per-instance
    // in `MeshTag` on EVERY batch of the model, billboard cards included (read by
    // `wow_model.wgsl`'s interior-prop lane). Its ordinary batches are steady indoors; a card still
    // distance-fades with its doodad, and the fade composes with the slot rather than clobbering it
    // (decision 0778). `None` everywhere else (exterior props fade; WMO groups use their own
    // per-submesh interior flag + batch class).
    interior_slot: Option<u16>,
    // The placement's **draw-set gate** — its fade sphere plus, for a WMO prop, the building
    // instance and the rooms that name it. `Some` on the world-static lanes, where the caller
    // builds it ONCE ([`super::fx::emitter_fade`]) and hands the same value to every rider of the
    // placement: the emitters, the ribbon trails, the glow lights — and, since 2059, the anim
    // host, which used to fabricate its own with `instance: None, room: None` and so judged a
    // particles-only WMO prop as exterior scene. Standing inside a sealed room that is
    // `ExteriorGate::Windows([])`, which "admits nothing": every meshless prop of the building the
    // camera was standing in was parked, its event track unscanned, and Stratholme's
    // burning-building `$DSL` never fired.
    //
    // `None` on the entity-hosted lane (a transport's props), which carries no `EmitterFade` at
    // all on purpose — a mover has no baked world point for one to measure from
    // (`benilla_app::entities::wmo_props`). The bare sphere below is what its mesh lane needs, and
    // `radius` is read for exactly that: in the `Some` arm the gate carries its own and this
    // parameter is never consulted again.
    fade: Option<&crate::particles::EmitterFade>,
    radius: f32,
    local_center: Vec3,
    // The model's authored **all-animation** bound in Bevy model-local space
    // ([`super::m2_anim_bound`]) — what an ANIMATED placement's submeshes are culled with instead of
    // their bind-pose mesh bound, which the joint palette has left behind (decision 1259). `None` for
    // WMO group geometry (no M2 header) and for a model with no authored box; a static placement
    // ignores it and keeps the tighter per-batch bound.
    anim_bound: Option<Aabb>,
    // `Some((model, now))` for an M2 placement: the doodad-animation gate (decision 0130) — an
    // animated model spawns joints + the skinned twins; `now` is the clock origin. `None` for WMO
    // group geometry.
    m2: Option<(&M2Model, f32)>,
    // UV-animated material registry (decision 0130 phase 3): a batch with a `uv_anim` loop registers
    // its material here so `tick_anim_materials` scrolls the shared offset while it is drawn.
    uv_reg: &mut crate::doodad_anim::UvAnimMaterials,
    // Animated-tint material registry (the M2Color RGB twin of `uv_reg`): a batch with an
    // `rgb_anim` loop registers its material here so `tick_anim_materials` re-samples the
    // shared tint each frame.
    tint_reg: &mut crate::doodad_anim::TintAnimMaterials,
    // The shared delta table both registries slot into (decision 1381) — registration allocates
    // here and bakes the slot into the material.
    anim_table: &mut crate::mat_anim_table::MatAnimTable,
    // **Is this placement a WMO doodad prop hanging off a streamed entity?** (The WMO-gameobject
    // lane — a transport's deck cargo and cabin furniture.) It is the LANE, where `card_owner`
    // below is only the anchor: that anchor is minted per placement and only when the model has
    // billboard batches at all, so it answers "does this prop have cards to follow", never "which
    // lane is this". Reading it as the lane is what left the marker below un-set on 133 of the
    // ship's 134 props.
    //
    // What it decides here: [`crate::entity_shade::DoodadDefLit`] on every batch, which is the
    // engine asserting its OWN invariant instead of trusting a caller to remember it (2047 — and
    // 2041's lesson one level up). A prop's light is its `CMapDoodadDef`'s; it is under the net
    // entity so it rides the deck, and the entity light node's descendant walk must pass it by.
    entity_hosted: bool,
    // `Some(anchor)` for a prop spawned ON a streamed entity (the WMO-gameobject path): a boneless
    // model's billboard cards FOLLOW this anchor (`BillboardCard::following` — the entity-path law,
    // decision 0153) instead of baking a world pivot, so they track the moving owner and
    // self-despawn with it; such cards are also excluded from the returned entity list (the caller
    // parents that list, and a card is a world root the billboard pass writes absolutely).
    // `None` for world-static placements (terrain), whose pivots never move.
    card_owner: Option<Entity>,
    // `Some` on the world-static streamer paths lane 1 covers (ADT doodads + WMO group
    // geometry): the production merge buffer and this placement's merge site (decision 1417,
    // `WOW_STATIC_MERGE`). `None` on the WMO-prop and moving-gameobject paths.
    mut merge: Option<(
        &mut super::super::merge::StaticMerge,
        super::super::merge::MergeSite<'_>,
    )>,
    // `Some((collector, site))` on the world-static streamer paths the retained pass may
    // take batches from (`WOW_STATIC_GX=1`, decisions 1429/1433; [`crate::static_gx`]):
    // ADT doodads (slice 1, cell items by owner tile), WMO group geometry (slice 2, region
    // items by portal-instance entity), and WMO props (B4, set-bucketed regions by the same
    // instance). `None` on the moving-gameobject path, and on a prop path whose placement
    // has no instance entity (no PVS identity — the site tallies those).
    mut staticgx: Option<(
        &mut crate::static_gx::StaticGx,
        crate::static_gx::GxSite<'_>,
    )>,
    // Returns the spawned entities + the per-batch map + what the anim host armed for this
    // placement, when the model animates: the joint set (bone-indexed) the emitter spawn rides
    // its host bone off (0130 phase 4), and the FILE sequence slot its variation roll landed
    // on, which the emitters' rate/gate tracks must be sampled against (decision 0760).
) -> SpawnedModel {
    // One gate for this placement, from here down: the caller's when it has one, else the bare
    // sphere (which is the same value `emitter_fade` would build from the same inputs — the
    // constructor is shared, so the two arms cannot drift). Every `radius`/world-centre read below
    // goes through it.
    let fade = match fade {
        Some(f) => f.clone(),
        None => {
            crate::particles::EmitterFade::sphere(radius, transform.transform_point(local_center))
        }
    };
    let kind = object.kind;
    let is_wmo = kind == ModelKind::Wmo;
    let mut out = Vec::with_capacity(submeshes.len());
    // The doodad-animation host (decision 0130 phase 1): an animated model — most placed instances
    // aren't, and stay on the static path below untouched — gets an anim-root + joint hierarchy +
    // player/drive from [`crate::doodad_anim::spawn_anim_host`]; its ordinary submeshes then draw
    // the skinned twin bound to those joints. Billboard batches keep today's camera-facing path
    // (their transform is the billboard system's, and their glow-pulse already rides
    // `BoneScaleAnim`). The host exists for every JOINT CONSUMER, not just skinned submeshes:
    // emitters and ribbons ride their bone's joint too (0130 phase 4), so a particles-only model
    // (0 render batches — the InstancePortal swirl, whose two emitters sit on the spinning rotor
    // bones) still needs its joints driven. Only a model with nothing but billboard cards and no
    // emitters/ribbons spawns no host.
    let mut host = m2
        .filter(|(m, _)| {
            submeshes.iter().any(|s| s.billboard.is_none())
                || !m.emitters.is_empty()
                || !m.ribbons.is_empty()
        })
        .and_then(|(m, _)| crate::doodad_anim::spawn_anim_host(commands, m, transform));
    // Captures freeze material-alpha clocks at 0 (deterministic frames) — read the env once.
    let mat_frozen = crate::dev_state::deterministic_run();
    let animated = host.is_some();
    let rig_root = host.as_ref().map(|h| h.root);
    if let Some(h) = &host {
        out.push(h.root);
    }
    let mut skinned_meshes: Vec<Entity> = Vec::new();
    // The parts the lazy-rig wake will promote static → skinned (decision 0863) — the ones that
    // got a [`crate::doodad_anim::SkinnedTwin`] below.
    let mut lazy_parts: Vec<Entity> = Vec::new();
    // The per-batch slot map (see [`SpawnedModel::by_batch`]) — every `continue` in the loop
    // below leaves a `None` in place instead of shifting later batches.
    let mut by_batch: Vec<Option<Entity>> = vec![None; submeshes.len()];
    // **Coplanar siblings ride ONE lane** — a batch whose M2 skin section is drawn by another
    // batch of this same model never diverts into a consolidator, and neither does that sibling.
    //
    // Two batches naming one section rasterize the SAME triangles ([`RenderSubmesh::section`]): a
    // base layer and the shine/reflect layer authored over it. The reference draws both from one
    // vertex array under depth-write + LEQUAL, so the later batch wins that coplanar tie EXACTLY
    // (`0x70c190`). The consolidators exclude on per-batch facts — env-mapped
    // UVs and the `0x10`/`0x08` depth flags (`static_gx::divert`), additive order (`merge`) — and a
    // shine layer carries all of them, so the pair got split: base into the retained/merged lane,
    // shine onto the entity path. Those two lanes reach the same world vertex by different
    // arithmetic (the bake resolves it on the CPU in f64 and re-centres; the entity shader rotates
    // it in f32 at local magnitude — `static_gx::bake`'s own note: "what remains against the entity
    // path is the GPU's own rotate-at-local-magnitude rounding, which no bake can undercut"). A few
    // ULPs is nothing between separate surfaces and everything between coplanar ones: the shine's
    // reverse-Z GreaterEqual test against its own base becomes a per-pixel coin flip that re-rolls
    // on every camera move — the ballista's bolt heads and shields dithering exactly the way B38's
    // awning did (decision 0680). Keeping the section whole is the only place the two depths are
    // equal by CONSTRUCTION; a depth bias would merely pick a winner for a tie we should never have
    // created. It costs 264 sections across 221 world doodads (`m2_shared_section`).
    let shared_geometry = shared_geometry(
        &submeshes
            .iter()
            .map(|s| s.geometry.section)
            .collect::<Vec<_>>(),
    );
    for (batch_idx, sub) in submeshes.iter().enumerate() {
        // The batch's app-built render form (decision 0834). Callers gate spawning on the forms
        // being complete, so a miss here is a broken contract — skip the batch rather than panic.
        let Some((stat_mesh, stat_aabb)) = forms.stat.get(batch_idx) else {
            continue;
        };
        // Every batch (WMO and M2) carries its authored order (index + 1) into the material's
        // transparent sort bias, so one model's coplanar layers draw in file order instead of
        // re-flipping a sort tie every frame (`model_render::BATCH_ORDER_SORT_EPS`). WMO batches
        // additionally ride it into the clip-z nudge that resolves coplanar layers in MOBA file
        // order — the byte-verified client behaviour (`0x6b4f10`/`0x6b5190`); `model_material`
        // gates that half on `is_wmo`.
        let batch_order = u16::try_from(batch_idx + 1).unwrap_or(u16::MAX);
        let interior = sub.interior || interior_slot.is_some();
        // A billboard card is culled by the SAME rule as any other batch — its material's `0x04`
        // flag, nothing else. This site used to force `|| sub.billboard.is_some()`, on the reasoning
        // that a card whose plane normal points away from the viewer would backface-cull to nothing;
        // that is precisely the mechanism the reference USES, and forcing it off is what drew the
        // stray solid triangles beside working particle effects (decision 0629, bugs B05/B34).
        // A billboard bone puts the model's +X toward the viewer, the cull is GL_BACK/CCW
        // (`0x70c2b3`), so a −X-facing card is never seen — and an author who wanted one seen
        // set `0x04` themselves: Elwynn's LampPost
        // carries a two-sided −X card AND a single-sided +X glow card, one model, both rules.
        let two_sided = sub.two_sided;
        // A lit interior M2 prop submesh (not a WMO group): it carries its SH-probe slot in
        // `MeshTag`, so the shader evaluates the room's probe instead of the sky base. **Billboard
        // batches are included** — a chain or glow card is a batch of the same model, and the
        // reference shades every batch of an object through one light node (decision 0778). They
        // were excluded here on a since-stale worry that the distance fade would overwrite the
        // slot: true before the 0355 re-lane, when the slot lived in the alpha bits, and false
        // after it — the fade writes through `mesh_tag::with_alpha`, which composes with the slot
        // (`debug_panel::visibility`). Excluding them left an interior card on an interior-mode
        // material with NO slot in its tag, which the shader decodes as slot **0** — whichever
        // probe won the streaming race, the same wrong-probe read the 0355 note below describes.
        let interior_probe = !is_wmo && interior_slot.is_some();
        // …and the narrower half: only a NON-card interior prop is steady indoors. A card still
        // distance-fades with its doodad (below — the halo must cull when the lamp does), so it
        // keeps its fade twin and its `DoodadFade`.
        let steady_interior_prop = interior_probe && sub.billboard.is_none();
        // **Does this batch need a material of its own?** (decision 1408.) A batch whose UV or tint
        // loop differs between sequences cannot share one: the registries are keyed by material,
        // so a shared one has no instance to ask which sequence is playing — and these placements
        // re-roll their variation independently every window (0768), so at any instant they are on
        // different slots. Keying the material by this placement's anim host gives each its own
        // registry entry, its own table row, and its own sequence. Everything else — every batch in
        // the world but a measured 28 on this lane, 49 corpus-wide of 24103 (`uvslotscan`) — keeps
        // the shared material it always had.
        //
        // No host ⇒ no sequence to read ⇒ share, and stay at the built seed: a per-sequence batch
        // on a model the content gate declined is exactly today's frozen behaviour, not a new one.
        let per_seq = sub.uv_seq.is_some() || sub.rgb_seq.is_some();
        let seq_owner = per_seq.then_some(rig_root).flatten();
        let cutout = model_material(
            mat_cache,
            materials,
            sub.texture.clone(),
            sub.blend,
            two_sided,
            is_wmo,
            interior,
            sub.emissive,
            sub.additive,
            false,
            sub.no_depth_write,
            sub.no_depth_test,
            sub.fog_policy,
            sub.env_map, // texture_unit_lookup > 2 ⇒ the runtime generates this batch's UVs
            shade,
            batch_order,
            sub.uv_anim.as_ref(),
            sub.rgb_anim.as_ref(),
            sub.wmo_batch,
            sub.sidn,
            sub.window,
            false, // the world streamer never spawns a skybox
            light,
            seq_owner,
        );
        // The blend twin for the distance-fade feather pass (reuse the cutout when already blend, or when
        // this is a non-fading interior prop). A MULTIPLY batch (Mod/Mod2x — the weapon-rack
        // ARMORREFLECT sheen) also reuses its steady self: its blend equation reads no alpha, so
        // no material swap can feather it — the fade rides the SOURCE COLOUR instead:
        // `wow_model.wgsl` lerps a multiply batch's colour toward the blend identity by the tag
        // alpha it already carries, which is the reference's own preset-5 mechanism (0865 built
        // it as a deviation; 1489 re-lawed it byte-faithful), so the rack sheen rides the
        // distance fade like everything else.
        let blend = if steady_interior_prop
            || matches!(
                sub.blend,
                ModelBlend::Blend | ModelBlend::Mod | ModelBlend::Mod2x
            ) {
            cutout.clone()
        } else {
            // The SOURCE blend (Opaque or AlphaKey here) rides into the twin: fade_variant builds
            // AlphaMode::Blend either way, and the source decides the twin's 224/255 cutout marker
            // (only an AlphaKey source alpha-tests while fading — decision 0842).
            model_material(
                mat_cache,
                materials,
                sub.texture.clone(),
                sub.blend,
                two_sided,
                is_wmo,
                interior,
                sub.emissive,
                sub.additive,
                true,
                sub.no_depth_write,
                sub.no_depth_test,
                sub.fog_policy,
                sub.env_map, // texture_unit_lookup > 2 ⇒ the runtime generates this batch's UVs
                shade,
                batch_order,
                sub.uv_anim.as_ref(),
                sub.rgb_anim.as_ref(),
                sub.wmo_batch,
                sub.sidn,
                sub.window,
                false, // the world streamer never spawns a skybox
                light,
                seq_owner,
            )
        };
        // The 1417 batch classification — ONE source for both the census tally and the
        // production divert below, so the population the census prints and the population the
        // merge takes can never drift apart.
        let class = crate::static_merge::BatchClass {
            excluded: animated
                || sub.billboard.is_some()
                || sub.alpha_anim.is_some()
                || seq_owner.is_some()
                || sub.uv_anim.is_some()
                || sub.rgb_anim.is_some(),
            interior_prop: interior_probe,
            order_free: matches!(sub.blend, ModelBlend::Opaque | ModelBlend::AlphaTest)
                && !sub.additive,
            never_fade: fade.radius > crate::model_fade::NEVER_FADE_RADIUS,
        };
        // A merged FADER blob lives permanently in the reference's own fading render state —
        // the blend twin (blend on, cutout ref on unfaded alpha, depth-write forced on) — with
        // per-vertex alpha; at fade 1.0 that state is pixel-identical to the steady cutout, so
        // nothing changes while it is steady and the feather is the reference's smooth
        // translucent ramp (1420, superseding 1419's ordered-dither erode, which quantized the
        // ramp to 16 visible steps — the director's report). Never-faders keep the cutout and
        // the opaque bins; a steady interior prop never fades at all, so it keeps the cutout
        // AND bakes an infinite sphere (its members carry no `DoodadFade` — the merged path
        // must not invent one).
        let merge_mat = if class.never_fade || steady_interior_prop {
            &cutout
        } else {
            &blend
        };
        let merge_sphere = if steady_interior_prop {
            Vec4::new(0.0, 0.0, 0.0, f32::INFINITY)
        } else {
            Vec4::from((fade.center, fade.radius))
        };
        if crate::static_merge::census_enabled() {
            let key = merge
                .as_ref()
                .and_then(|(_, site)| site.census_key(batch_idx, merge_mat, &transform));
            crate::static_merge::tally(&class, is_wmo, sub.geometry.positions.len(), key);
        }
        // The retained-pass divert (`WOW_STATIC_GX=1`, decisions 1429/1431/1433): an
        // eligible order-free static batch leaves bevy_pbr entirely — no entity, no blob;
        // `static_gx` draws it from retained buffers (per-cell for ADT doodads,
        // per-instance group-bucketed for WMO group geometry — slice 2, per-instance
        // set-bucketed for WMO props — B4). B2 admits DOODAD FADERS too: the batch rides
        // in with its exile seed (the placement identity + handle clones of this exact
        // bundle), and the scan respawns it through the entity path for the feather band.
        // Checked BEFORE the merge divert (armed, this lane owns the populations lanes 1+3
        // would have taken); a refused batch — the collector's own exclusions or a
        // site/kind mismatch — falls through to the ordinary paths below, slot held like
        // every other divert. Each site gates its own class: the doodad lane requires the
        // seed for faders, the WMO lane is never-fade by construction (∞ radius), and the
        // prop lane admits steady interior props + never-fade exterior props while an
        // exterior FADER prop stays on the entity path (the exile protocol has no prop
        // shape — that keep is today's default look) and is TALLIED, never silent.
        // The census label for a batch that stays on the entity path (`EntityPathWhy`):
        // refined below as each divert declines it.
        let mut why: &'static str = "gx-off";
        if let Some((gx, site)) = staticgx.as_mut() {
            let facts = if !crate::static_gx::enabled() {
                None
            } else if shared_geometry[batch_idx] {
                why = "shared-geometry";
                None
            } else {
                why = match site {
                    crate::static_gx::GxSite::Doodad { .. } if is_wmo => "doodad-site-wmo",
                    crate::static_gx::GxSite::Doodad { .. } if class.excluded => "no-merge-anim",
                    crate::static_gx::GxSite::Doodad { .. } if !class.merges() => {
                        "no-merge-transparent"
                    }
                    crate::static_gx::GxSite::Doodad { .. } if class.interior_prop => {
                        "interior-prop"
                    }
                    crate::static_gx::GxSite::Doodad { .. } => "fader-lane-off",
                    crate::static_gx::GxSite::Wmo { .. } if !is_wmo => "wmo-site-m2",
                    crate::static_gx::GxSite::Wmo { .. } if class.excluded => "wmo-no-merge-anim",
                    crate::static_gx::GxSite::Wmo { .. } if !class.merges() => {
                        "wmo-no-merge-transparent"
                    }
                    crate::static_gx::GxSite::Wmo { .. } => "wmo-no-group",
                    crate::static_gx::GxSite::Prop { .. } if is_wmo => "prop-site-wmo",
                    crate::static_gx::GxSite::Prop { .. } if class.excluded => "prop-no-merge-anim",
                    crate::static_gx::GxSite::Prop { .. } if !class.merges() => {
                        "prop-no-merge-transparent"
                    }
                    crate::static_gx::GxSite::Prop { .. } => "exterior-fader-prop",
                };
                match site {
                    crate::static_gx::GxSite::Doodad { owner }
                        if !is_wmo && class.merges() && !class.interior_prop =>
                    {
                        // `fade_seed`, not `fade`: the placement's draw-set gate is in scope
                        // here under that name, and shadowing it with the retained pass's
                        // per-batch seed reads as the same thing twice.
                        let fade_seed = (!class.never_fade
                            && !crate::static_gx::fade_lane_disabled())
                        .then(|| crate::static_gx::GxFadeSeed {
                            radius: fade.radius,
                            local_center,
                            stat_mesh: stat_mesh.clone(),
                            aabb: *stat_aabb,
                            cutout: cutout.clone(),
                            blend: blend.clone(),
                        });
                        // Never-fade admits bare; a fader admits only WITH its seed (the
                        // fade lane lever empties the seed, sending faders back to the
                        // entity path).
                        (class.never_fade || fade_seed.is_some())
                            .then_some((*owner, None, None, fade_seed))
                    }
                    crate::static_gx::GxSite::Wmo { instance, groups }
                        if is_wmo && class.merges() =>
                    {
                        groups.get(batch_idx).map(|&g| {
                            (
                                (0, 0), // WMO items release by instance death, never by tile
                                Some(crate::static_gx::GxWmoBatch {
                                    instance: *instance,
                                    group: g,
                                    interior,
                                    class: sub.wmo_batch,
                                    sidn: sub.sidn,
                                    window: sub.window,
                                    batch_order,
                                }),
                                None,
                                None,
                            )
                        })
                    }
                    crate::static_gx::GxSite::Prop {
                        instance,
                        groups,
                        slot,
                    } if !is_wmo && class.merges() => {
                        if steady_interior_prop || class.never_fade {
                            Some((
                                (0, 0), // prop items release by instance death too
                                None,
                                Some(crate::static_gx::GxPropBatch {
                                    instance: *instance,
                                    groups: std::sync::Arc::clone(groups),
                                    slot: *slot,
                                }),
                                None,
                            ))
                        } else {
                            gx.tally_prop_declined(false); // an exterior fader prop
                            None
                        }
                    }
                    _ => None,
                }
            };
            if let Some((owner, wmo, prop, fade)) = facts {
                why = "gx-declined";
                if gx.divert(crate::static_gx::GxBatch {
                    geometry: &sub.geometry,
                    transform,
                    object,
                    aabb: *stat_aabb,
                    owner,
                    texture: sub.texture.clone(),
                    blend: sub.blend,
                    two_sided,
                    unlit: sub.emissive,
                    fog_policy: sub.fog_policy,
                    env_map: sub.env_map,
                    no_depth_write: sub.no_depth_write,
                    no_depth_test: sub.no_depth_test,
                    shade,
                    wmo,
                    prop,
                    fade,
                }) {
                    continue;
                }
            }
        }
        // The production consolidation divert (`WOW_STATIC_MERGE=1`, 1417/1418): an order-free
        // static doodad batch — faders included, their fade rides the baked sphere through the
        // shader's WOW_MERGED_FADE lane — merges into its site's blob instead of spawning. Its
        // `by_batch` slot stays `None`; a refused divert (a WMO/prop site — 1418's verdict)
        // falls through and spawns individually.
        if let Some((merge, site)) = merge.as_mut() {
            if crate::terrain_stream::merge::merge_enabled()
                && !shared_geometry[batch_idx]
                && class.merges()
                // The fader lane is OPT-IN (`WOW_MERGE_FADERS=1`, decision 1423): a fader —
                // neither never-fade nor a steady interior prop — spawns per-entity by
                // default; a cell-granular transparent draw with depth-write depth-kills
                // per-entity faders behind its translucent pixels (the popping lamppost).
                && (class.never_fade
                    || steady_interior_prop
                    || crate::terrain_stream::merge::merge_faders_enabled())
                && merge.divert(
                    site,
                    batch_idx,
                    merge_mat,
                    &sub.geometry,
                    transform,
                    merge_sphere,
                    sub.blend,
                    kind,
                    object,
                )
            {
                continue;
            }
        }
        // Register both variants' materials for the per-frame UV scroll (idempotent per material —
        // every instance of the batch lands on the same deduped handle).
        // A period-0 loop is a constant the material seed already wrote (`sample(0.0)` IS its
        // forever value) — registering it would buy a per-frame re-write of the same number
        // (1375), so only a real loop enters the registry.
        // The per-placement lane first: its set replaces the shared loop outright, and it only
        // arms when this placement actually has a host to read a sequence from.
        if let (Some(seqs), Some(host)) = (sub.uv_seq.as_ref(), seq_owner) {
            for id in [cutout.id(), blend.id()] {
                crate::doodad_anim::register_uv(
                    uv_reg,
                    anim_table,
                    materials,
                    id,
                    crate::doodad_anim::UvLoop::PerSeq {
                        seqs: seqs.clone(),
                        host,
                    },
                );
            }
        } else if let Some(anim) = sub.uv_anim.as_ref().filter(|a| a.period > 0.0) {
            for id in [cutout.id(), blend.id()] {
                crate::doodad_anim::register_uv(
                    uv_reg,
                    anim_table,
                    materials,
                    id,
                    crate::doodad_anim::UvLoop::Shared(Some(anim.clone())),
                );
            }
        }
        // …and for the per-frame tint re-sample (the animated M2Color RGB, same shared clock —
        // the same invisible seq-band phase divergence as the UV scroll, recorded there).
        if let (Some(seqs), Some(host)) = (sub.rgb_seq.as_ref(), seq_owner) {
            for id in [cutout.id(), blend.id()] {
                crate::doodad_anim::register_tint(
                    tint_reg,
                    anim_table,
                    materials,
                    id,
                    crate::doodad_anim::TintLoop::PerSeq {
                        seqs: seqs.clone(),
                        host,
                    },
                );
            }
        } else if let Some(anim) = sub.rgb_anim.as_ref().filter(|a| a.period > 0.0) {
            for id in [cutout.id(), blend.id()] {
                crate::doodad_anim::register_tint(
                    tint_reg,
                    anim_table,
                    materials,
                    id,
                    crate::doodad_anim::TintLoop::Shared(anim.clone()),
                );
            }
        }
        // `MeshTag` is the per-instance lighting/fade scalar: an interior prop carries its SH-probe
        // slot index here (the shader evaluates the folded probe instead of the sky base);
        // everything else carries the distance-fade alpha (`1.0` = opaque, the no-fade default the
        // fade system overwrites). The slot goes through the ONE typed constructor
        // (`mesh_tag::probe_bits` — bits 16-29 since the 0355 re-lane): this site kept the old
        // bits-0..=15 write through that re-lane, so every static interior prop read probe slot 0,
        // taking whichever probe won the streaming race — the director's inn-doodad regression.
        // ONE decision for "does this batch's payload carry a probe slot", read twice below —
        // once for the tag's bits and once for the component that SAYS so
        // ([`crate::mesh_tag::InteriorProbePayload`]). Two independent predicates could drift,
        // and a reader that disagrees with the writer about which payload a part is on is
        // exactly B373.
        let probe_slot = interior_slot.filter(|_| interior_probe);
        let mesh_tag = match probe_slot {
            Some(slot) => MeshTag(crate::mesh_tag::probe_bits(slot)),
            None => MeshTag(alpha_bits(1.0)),
        };
        // A billboard batch (glow card / chain) faces the camera each frame, so its transform is owned
        // by the billboard system. It still distance-fades with its doodad (same `radius` band) — the
        // fade only touches the material/`MeshTag`, not the transform — so the bright halo culls when the
        // lamp does instead of hanging in the air far past it. The mesh is centred at the pivot, so the
        // fade centre is `ZERO` (→ the entity's world translation = the pivot).
        let (entity, fade_center) = if let Some(info) = &sub.billboard {
            // An animated doodad's card rides its billboard bone's live anchor (the swinging
            // lamp's glow follows the swing — 0153 follow-up; the anchor frame bakes the pivot,
            // the 0130 rig identity carried into the collapsed lane by 1365). A static doodad
            // keeps the fixed placement-baked pivot.
            let card = match host.as_mut().and_then(|h| h.anchor(commands, info.bone)) {
                Some(anchor) => BillboardCard::following_joint(info, anchor),
                None => match card_owner {
                    Some(anchor) => BillboardCard::following(info, anchor),
                    None => BillboardCard::new(info, transform),
                },
            };
            let mut card_entity = commands.spawn((
                Mesh3d(stat_mesh.clone()),
                MeshMaterial3d(cutout.clone()),
                Transform::from_translation(transform.transform_point(info.pivot)),
                ModelPart {
                    kind,
                    blend: sub.blend,
                },
                crate::model_render::EntityPathWhy(why),
                // The picker's triangles (decision 0857): the render forms are `RENDER_WORLD`-only,
                // so the inspector/probe rays read the model's resident geometry. The caster
                // centres a card at its pivot, the same bake the render form draws with.
                crate::interact::PickMesh(sub.geometry.clone()),
                mesh_tag,
                card,
            ));
            // The build-time Aabb, inserted explicitly: the static form is `RENDER_WORLD`-only,
            // so Bevy's `calculate_bounds` can race extraction — and the exterior cull fails
            // OPEN on a missing bound (0832's rule, extended to the model lane). `NoAutoAabb`
            // says the bound is OURS: see the ordinary-part insert below for what derives it
            // otherwise, and what that cost.
            if let Some(aabb) = stat_aabb {
                card_entity.insert((*aabb, NoAutoAabb));
            }
            (card_entity.id(), Vec3::ZERO)
        } else {
            // An animated doodad's ordinary submesh spawns on the STATIC form with its skinned
            // twin waiting beside it (`SkinnedTwin`): the palette slot no longer exists at spawn
            // — the draw gate allocates it at the placement's first wake and swaps the mesh in
            // (decision 0863, `doodad_anim::lazy`). The twin comes from the app-built forms
            // (0834); a lane that didn't request it (or a contract break) simply never promotes,
            // rather than arm the picker with a joint-less "skinned" mesh.
            let skinned_mesh = animated
                .then(|| forms.skin.and_then(|s| s.get(batch_idx)).cloned())
                .flatten();
            let mut part_entity = commands.spawn((
                Mesh3d(stat_mesh.clone()),
                MeshMaterial3d(cutout.clone()),
                transform,
                ModelPart {
                    kind,
                    blend: sub.blend,
                },
                crate::model_render::EntityPathWhy(why),
                // The picker's triangles (decision 0857) — same rule as the card above.
                crate::interact::PickMesh(sub.geometry.clone()),
                mesh_tag,
            ));
            // Every part now spawns static, so every part carries the build-time Aabb (the
            // RENDER_WORLD rule above).
            //
            // For an ANIMATED placement that bind-pose bound is a lie, because the joints move the
            // vertices while this entity's transform stays at the placement origin. Widen to the
            // model's authored all-animation box (decision 1259) — the union, never the replacement,
            // so the 152 corpus models whose authored box does not fully contain their own bind pose
            // still bound their geometry. A billboard card is exempt: its entity transform FOLLOWS
            // its joint every frame, so its own small bound travels with what it draws.
            //
            // **`NoAutoAabb` is what makes that bound survive** (decision 1261). Bevy's
            // `calculate_bounds` runs TWO queries: one that inserts a bound where there is none,
            // and one that **overwrites an existing bound** on `Changed<Mesh3d>`. The lazy rig
            // swaps `Mesh3d` static → skinned at this placement's first draw-gate wake
            // (`doodad_anim::lazy`), and the skinned twin shares the BIND-POSE geometry — so the
            // second query recomputed the very box 1259 had just widened, and the birds went back
            // to blinking a second after they streamed in. The bound here is authored, not derived;
            // this component is how that is said.
            if let Some(aabb) =
                cull_bound(stat_aabb.as_ref(), animated.then_some(anim_bound).flatten())
            {
                part_entity.insert((aabb, NoAutoAabb));
            }
            if let Some(sm) = skinned_mesh {
                part_entity.insert(crate::doodad_anim::SkinnedTwin {
                    skinned: sm,
                    stat: stat_mesh.clone(),
                });
                lazy_parts.push(part_entity.id());
            }
            let entity = part_entity.id();
            if let Some(root) = rig_root {
                commands
                    .entity(entity)
                    .insert(crate::rig_palette::RigPart(root));
                // The draw-gate list is about visibility, not skinning — the bind-pose fallback
                // part still represents the placement.
                skinned_meshes.push(entity);
            }
            (entity, local_center)
        };
        by_batch[batch_idx] = Some(entity);
        // This batch's payload is a PROBE SLOT, said as a component so the exterior-payload
        // writer can see it (`mesh_tag::InteriorProbePayload`). Cards included, for the same
        // reason they carry the slot at all: they are batches of the same model, shaded through
        // the same light node (0778) — and `entity_shade`'s card pass reaches a card by walking
        // UP from its owner, a route its descendant-walk guard never covers.
        if probe_slot.is_some() {
            commands
                .entity(entity)
                .insert(crate::mesh_tag::InteriorProbePayload);
        }
        // …and, on the entity-hosted lane, that this batch's light is its own doodad def's rather
        // than its host's. Asserted here rather than by the caller so the rule cannot be half
        // applied: this is the site that knows about EVERY batch, cards included, and a card is
        // exactly what the caller cannot reach (it is a world root kept out of the returned list,
        // and the shade writer's card pass finds it by walking UP to the host).
        if entity_hosted {
            commands
                .entity(entity)
                .insert(crate::entity_shade::DoodadDefLit);
        }
        // Animated material alpha (decision 0130 phase 2): the rare batch whose colour-alpha/weight
        // tracks animate (fire flicker) or constantly dim gets its per-instance sampler; the
        // visibility authority composes the value into the render-alpha tag + the A ≤ 0 cull.
        // Captures freeze the clock at 0 (deterministic frames). A lit interior prop composes the
        // same way since the 0355 re-lane gave the probe-slot payload its own alpha field (bits
        // 0..=15) — the 0130-era "partial alpha can't show on the colour payload" deferral is
        // collected (bug B30: the Undercity lightshaft's authored 0.10/0.05 dimming).
        // The scan marker (1375), same predicate as the registration above — so a marked row
        // always has a registry key to match, and `tick_anim_materials` never has to visit an
        // unmarked one.
        if sub.uv_anim.as_ref().is_some_and(|a| a.period > 0.0)
            || sub.rgb_anim.as_ref().is_some_and(|a| a.period > 0.0)
            || seq_owner.is_some()
        {
            commands
                .entity(entity)
                .insert(crate::doodad_anim::AnimMatPart);
        }
        if let Some(anim) = &sub.alpha_anim {
            commands
                .entity(entity)
                .insert(crate::doodad_anim::MatAnim::new(
                    anim.clone(),
                    m2.map(|(_, now)| now).unwrap_or_default(),
                    mat_frozen,
                ));
        }
        // Distance fade for everything except a lit interior prop (whose `MeshTag` carries colour, and
        // which is steady indoors). Adding `DoodadFade` is what makes the fade system drive the tag.
        if !steady_interior_prop {
            commands.entity(entity).insert(DoodadFade {
                radius: fade.radius,
                local_center: fade_center,
                cutout,
                blend,
            });
        }
        // An entity-following card manages its own lifecycle (`face_billboards` despawns it with
        // its owner) and must stay a world root — keep it out of the caller's parent/despawn list.
        if sub.billboard.is_none() || card_owner.is_none() {
            out.push(entity);
        }
    }
    // Arm the draw gate: animation runs iff any of these submeshes is drawn (`doodad_anim`) —
    // or, for a meshless (particles-only) host, iff the placement's fade sphere is in the draw
    // set (the same 0171 law its emitters gate on).
    let arm = host.as_ref().and_then(|h| h.seq.map(|_| h.root));
    let placement_host = host.map(|mut h| {
        let now = m2.map(|(_, now)| now).unwrap_or_default();
        // Pre-mint the fx consumers' anchors (decision 1365) while the pose buffer is still in
        // hand: exactly the bones the model authors emitters/ribbons on, nothing speculative —
        // the fx spawns that run after this only look the entities up ([`PlacementHost::anchor`]).
        if let Some((m, _)) = m2 {
            for bone in m
                .emitters
                .iter()
                .map(|e| e.def.bone)
                .chain(m.ribbons.iter().map(|r| r.def.bone))
            {
                h.anchor(commands, bone);
            }
        }
        commands.entity(h.root).insert(DoodadAnimHost {
            meshes: skinned_meshes,
            // The caller's one gate, not a second construction of it (2059).
            fade: fade.clone(),
            clip: h.clip,
            armed_at: now,
            // Born already expired, so the first frame runs the holder setup's `variationIdx = -1`
            // arm over the loader's var-0 seed — the reference's own two-stage load (decision 0768).
            window_hi: f32::NEG_INFINITY,
            anim_id: h.anim_id,
            // Born PARKED (decision 0863): the spawn frame's `Visibility` is the default-visible
            // lie (the fade/cull authorities haven't classified the fresh parts yet), and the
            // gate's promote requires a drawn frame ON TOP of `active` — so the first real
            // verdict, not the spawn race, decides the slot. The gate flips this true the same
            // first pass either way, so pose/resume semantics are unchanged.
            active: false,
            parked_at: now,
        });
        // The lazy palette rig (decision 0863): what the draw gate's first wake allocates and
        // promotes. Only when the placement has skinnable parts at all — an emitter-only host
        // (chimney smoke) never takes a slot, which under the eager design it wasted one on.
        if !lazy_parts.is_empty() {
            commands.entity(h.root).insert(crate::doodad_anim::LazyRig {
                bones: h.bones(),
                ibp: h.inverse_bindposes.clone(),
                parts: lazy_parts,
            });
        }
        let root = h.root;
        let anchors = h.finish(commands);
        PlacementHost { anchors, root, arm }
    });
    SpawnedModel {
        entities: out,
        by_batch,
        host: placement_host,
    }
}

/// The `Aabb` a placed submesh is culled with: its build-time bind-pose bound, widened by the
/// model's authored all-animation box when this placement animates (decision 1259).
///
/// It is a **union**, not a swap, for one measured reason: 152 of the 9315 M2s that carry geometry
/// author a header box that does not fully contain their own bind-pose vertices (worst case
/// `Spells\Teleport.m2`, 19.9 yd short). The reference never notices — it tests the box's
/// *circumsphere*, which swallows the shortfall — so a bare swap would be the one way this change
/// could cull geometry the reference draws.
fn cull_bound(stat: Option<&Aabb>, anim: Option<Aabb>) -> Option<Aabb> {
    match (stat, anim) {
        (Some(s), Some(a)) => Some(Aabb::from_min_max(
            Vec3::from(s.min().min(a.min())),
            Vec3::from(s.max().max(a.max())),
        )),
        (Some(s), None) => Some(*s),
        (None, a) => a,
    }
}

/// Which batches share their triangles with another batch of the same model — the consolidator
/// gate's predicate, over the batches' [`benilla_formats::RenderSubmesh::section`] indices in batch
/// order. `true` at `i` means batch `i` names a section some other batch also names.
///
/// `None` (every WMO batch — no section concept) never shares: the WMO lanes keep their own
/// MOBA-order law and are not what this closes. The list is a model's batch count long — tens at
/// the very most — so the quadratic scan is cheaper than any map.
fn shared_geometry(sections: &[Option<u16>]) -> Vec<bool> {
    sections
        .iter()
        .map(|s| s.is_some() && sections.iter().filter(|o| *o == s).count() > 1)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug's own numbers (`benilla-extract animboundscan`, `World\critter\birds\Bird01.m2`):
    /// a 1.2 × 1.8 × 0.23 yd bind-pose box, and an authored all-animation box 67 × 18 × 7 yd that
    /// reaches ~37 yd past it. Culled by the bind pose, the bird is dropped while its drawn
    /// geometry is still on screen.
    #[test]
    fn an_animated_placements_bound_covers_the_whole_flight_path() {
        // Bevy-space conversions of Bird01's two boxes (WoW (x,y,z) -> Bevy (-y, z, -x)).
        let bind = Aabb::from_min_max(
            Vec3::new(-0.863, 9.359, -3.536),
            Vec3::new(0.915, 9.586, -2.366),
        );
        let authored = Aabb::from_min_max(
            Vec3::new(-4.219, 8.816, -30.547),
            Vec3::new(13.571, 16.019, 36.605),
        );
        let widened = cull_bound(Some(&bind), Some(authored)).expect("a bound");
        // Every corner of the authored box is inside the widened bound. `Aabb` stores
        // centre/half-extents, so the reconstructed corners carry a float epsilon — hence the
        // tolerance, which is ~7 orders of magnitude below anything a cull can see.
        const EPS: f32 = 1e-4;
        assert!((widened.min() - EPS).cmple(authored.min()).all());
        assert!((widened.max() + EPS).cmpge(authored.max()).all());
        // …and the bind-pose bound alone was ~37 yd short of it along the flight axis.
        assert!(authored.max().z - bind.max().z > 36.0);
    }

    /// The union arm, not a swap: an authored box that fails to contain the bind pose (152 corpus
    /// models do) must not shrink the bound.
    #[test]
    fn a_short_authored_box_never_shrinks_the_bound() {
        let bind = Aabb::from_min_max(Vec3::splat(-5.0), Vec3::splat(5.0));
        let authored = Aabb::from_min_max(Vec3::new(-40.0, -1.0, -1.0), Vec3::new(40.0, 1.0, 1.0));
        let b = cull_bound(Some(&bind), Some(authored)).expect("a bound");
        assert!(Vec3::from(b.min()).abs_diff_eq(Vec3::new(-40.0, -5.0, -5.0), 1e-4));
        assert!(Vec3::from(b.max()).abs_diff_eq(Vec3::new(40.0, 5.0, 5.0), 1e-4));
    }

    /// A static placement is untouched — it keeps the tighter per-batch bind-pose bound.
    #[test]
    fn a_static_placement_keeps_its_per_batch_bound() {
        let bind = Aabb::from_min_max(Vec3::splat(-1.0), Vec3::splat(1.0));
        let b = cull_bound(Some(&bind), None).expect("a bound");
        assert!(Vec3::from(b.min()).abs_diff_eq(Vec3::splat(-1.0), 1e-4));
        assert!(Vec3::from(b.max()).abs_diff_eq(Vec3::splat(1.0), 1e-4));
    }

    /// The ballista's own shape: section 10 drawn twice (base + shine) and section 4 drawn once.
    /// Both members of the pair are flagged, not just the refusable one — the whole point is that
    /// they travel together, so gating only the shine would leave the base in the retained lane
    /// and the split exactly where it was.
    #[test]
    fn both_batches_of_a_shared_section_are_flagged() {
        let flags = shared_geometry(&[Some(4), Some(10), Some(10), Some(7)]);
        assert_eq!(flags, vec![false, true, true, false]);
    }

    /// WMO batches carry no section, and `None == None` would otherwise make every WMO batch in a
    /// group "shared" with every other — pulling the whole WMO population out of the retained lane.
    #[test]
    fn section_less_batches_never_share() {
        assert_eq!(shared_geometry(&[None, None, None]), vec![false; 3]);
        assert_eq!(
            shared_geometry(&[None, Some(0), Some(0), None]),
            vec![false, true, true, false]
        );
    }
}
