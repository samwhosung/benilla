//! Entity ground-shade (decision 0173): units, players, and GameObjects sample the terrain MCSH under
//! them **dynamically** and dim their sun term when standing in baked ground shadow: a spawned
//! unit/player/GameObject carries the SAME 2.5-lit / 0.5-MCSH-shadowed chain as an ADT doodad, driven by
//! a per-frame MCSH sample at the object's node position and a linear intensity ramp (`0x69e770`, the
//! step constant `[0x810808] = 3.3333`/s), not a static spawn-time bake.
//!
//! **The chain applies to units, and 0814 restored it after 0809 wrongly took it away.** The reference
//! has TWO real delivery states for a unit's committed light, and which one a given unit is in is a
//! *lifecycle* fact, not a category fact. Delivery splits at `0x672a20`: with `[model+0x3c0] != 0` the
//! node applies (`0x6a7300` multiplies the diffuse by the ramped `[+0xa4]` — the 2.5/0.5 chain this
//! file models); with `[model+0x3c0] == 0` the null fallback commits the raw day/night pair with no
//! intensity multiply at all — a hardwired ×1.0, position-independent, never sampling MCSH.
//! Registration fires on a model-set with the node present (`Node::SetModel 0x6716f0` ←
//! `0x613cf0`/`0x613d80`: equip / display-id / shapeshift); node birth (`0x670db0` from Activate
//! `0x613e10`) passes model arg 0, so its birth-time registration gate `670fca` is skipped and a unit
//! is born UNREGISTERED.
//!
//! **We model REGISTERED, because that is the steady state of anything we draw.** Every unit benilla
//! renders has had its display model applied — which is exactly the node-present model-set that
//! registers it — so unregistered is the pre-display transient, not the resting state. Two
//! reference frames bracket this and disagree with each other: a standing Stormwind player
//! commits **×2.5** (registered, MCSH-lit) while a running Northshire player commits ×1.0.
//! Which discriminant governs is open, a benilla-side observable — so a single hardwired value
//! cannot be read off it in either direction, and 0809 read off the ×1.0 half. The director's eye
//! settles the tie the way the ×2.5 frame does: a
//! character standing in shade reads visibly dimmer than one in sun, and flattening that was wrong.
//!
//! The null fallback is therefore real, verified, and **deliberately not modelled** — see 0814. If we
//! ever want it, it is a lifecycle flag flipped by the display-model apply, never a per-kind constant.
//!
//! Structure mirrors the reference's one-light-node-per-object: [`GroundShade`] lives on the **net
//! entity root** (the `[obj+0xe0]` twin), sampled at the root's feet and ramped there; the resulting
//! shade byte is written to every M2 part in the root's descendant tree — body submeshes, and held
//! items/helm/shoulders hanging off joint entities — via the `MeshTag` shade field (`mesh_tag`), so a
//! weapon dims with its wielder rather than sampling independently at the hand (the real client lights
//! attachments from the owner's node; independent sampling would flicker at shadow edges mid-swing).
//!
//! Interplay: the interior classifier ([`crate::interior`]) owns a part's tag while it stands in a WMO
//! room (an SH-probe slot — no sun indoors, so no shade either); this system skips those parts and
//! runs after the classifier to re-assert the byte over its exterior reclaim. Fades own the alpha field
//! only (they write through `with_alpha`), so shade rides through appear/despawn/zoom feathering.
//!
//! **The descendant tree is a TRANSFORM relation, not a light one, and the difference is a bug we
//! shipped** (B373). A WMO-display GameObject's doodad props — a transport's cabin furniture — are
//! parented under the net entity so they sail with the deck (decision 0474), but their light is
//! their own baked MODD colour folded into an SH probe, the reference's `CMapDoodadDef` provider
//! (`0x6a8050`) rather than the WENTITY node this file models. They were nonetheless in the walk,
//! and since the shade byte overlaps the probe slot in bits 6..=13, every one of them was pushed
//! onto a *different* probe: a neighbouring prop's light where the renamed index happened to be
//! live, and an unallocated — zeroed — row where it was not, which draws solid black. Both write
//! sites now ask [`probe_payload`], the payload question rather than the classifier question.

use benilla_assets::AdtTile;
use bevy::mesh::MeshTag;
use bevy::prelude::*;

use crate::interior::{classify_entity_interior, InteriorLit};
use crate::mesh_tag::{exterior_payload, shade_of, with_shade, InteriorProbePayload};
use crate::terrain_stream::{doodad_ground_shade, ShadeResolve, TerrainStreamer};

// Decision 0354 generalized this file from "the MCSH ground-shade byte" to the entity light
// node's CPU ramp pair: `t` (the intensity chase — the tag byte the exterior SH lane scales by)
// and the ambient word chase the interior bake fold consumes. The MCSH sample now only picks the
// OUTDOOR target; the classifier's interior verdict overrides it with the day/night point.

/// How fast the shade mix `t` (0 = intensity 2.5, 1 = 0.5) moves toward its target: the binary's
/// linear step `[0x810808] = 3.3333` **intensity-units/s**, expressed on `t` by dividing by the 2.0-wide
/// span the mix covers. The rate is on the intensity axis and is unchanged by [`LIT_T`] — what 0821
/// changed is the *distance travelled* (1.0 → 0.5 rather than 2.5 → 0.5), so a lit→shaded transition
/// now takes 0.15 s of fully visible movement instead of 0.6 s that was 75 % invisible.
const SHADE_RAMP_PER_SEC: f32 = 3.3333 / 2.0;

/// Squared distance (yd²) the root must move before the MCSH bit is re-sampled — same gate as the
/// interior classifier: a standing NPC costs a position compare and nothing else. (MCSH texels are
/// ~0.5 yd, so half a yard of hysteresis is at the sample's own resolution.)
const RESAMPLE_DIST_SQ: f32 = 0.25;

/// The ambient word's ramp rate — the binary's `[0x810804] = 2.0` colour-units/s (`0x69e770`'s
/// FIRST chase, `[+0x9c]` → `[+0xf4]` — the intensity
/// chase runs at its own 3.3333).
const AMBIENT_RAMP_PER_SEC: f32 = 2.0;

/// The shade mix `t` encoding the DAY/NIGHT intensity 1.0: `intensity = mix(2.5, 0.5, t)` ⇒
/// `t = 0.75`. **Two distinct reference mechanisms land on this same value** and only ONE of them is
/// modelled here, so keep them apart if either moves: the live one is an indoor entity's node target
/// (the interior `[+0xf8] = 1.0`, written at `69e36b` behind the `[+0xc]&2` interior gate); the other
/// is `0x672a20`'s null-node fallback for an unregistered unit (no intensity multiply at all), which
/// 0814 records as real but deliberately unmodelled. A value shared by a live law and a shelved one is
/// exactly how 0809 talked itself into pinning every unit here.
const DAYNIGHT_T: f32 = 0.75;

/// The LIT outdoor target (decision 0821). The reference's lit target is intensity **2.5**, and this is
/// **1.0** — not because the 2.5 is wrong, but because our shader cannot express it: `wow_model.wgsl`
/// caps the gain with `min(intensity, 1.0)`, so every value from 2.5 down to 1.0 renders *identically*.
///
/// **Clamp the TARGET, not the gain.** With the cap on the gain, `t` ramped 0 → 1 over 0.6 s while the
/// first 0.75 of that journey (0.45 s) was pinned at 1.0 and therefore **invisible** — the shade change
/// read as a dead pause followed by a snap, which is what the director reported walking from sun into
/// shade ("delayed by ~1 s"). Aiming the chase at the value the renderer can actually show removes the
/// invisible stretch without touching a single rendered pixel of the settled states: lit still commits
/// 1.0 (the cap was already delivering that) and shadowed still commits 0.5.
///
/// This is therefore a faithful ramp over an unfaithful *range*, and the range is the open item: **the
/// day the `min(I, 1)` cap is lifted, this constant goes back to 0.0** and the full 2.5 → 0.5 sweep
/// becomes visible on its own. The two must move together — see 0821, and 0803 §3 for why the cap is
/// still there.
const LIT_T: f32 = 0.75;

/// Settled-ramp epsilon (on `t` and each ambient channel) — under half a tag/colour byte.
const RAMP_EPS: f32 = 1.0 / 640.0;

/// The per-entity light-node state, on the net entity **root** (unit / player / GameObject) — the
/// CPU twin of the reference's per-object light node (decision 0354): the intensity chase
/// (`[+0xa4]`→`[+0xf8]`, held as the normalized mix `t` over the 2.5→0.5 span) and the ambient
/// word chase (`[+0x9c]`→`[+0xf4]`) — the pair `0x69e770` steps every frame.
#[derive(Component)]
pub struct GroundShade {
    /// Current shade mix (1 = intensity 0.5; [`LIT_T`] = the lit 1.0 our shader can express, and also
    /// [`DAYNIGHT_T`]'s day/night 1.0 — the two coincide while the gain cap stands) — what the
    /// parts' tags show, and what the interior bake fold scales its diffuse word by.
    t: f32,
    /// Where `t` is ramping to ([`LIT_T`]/1 from the last MCSH sample outdoors; [`DAYNIGHT_T`] indoors).
    target: f32,
    /// Root position at the last sample (the movement gate).
    last_pos: Vec3,
    /// Whether the first sample landed (it snaps `t = target`; a spawn never plays a ramp-in).
    sampled: bool,
    /// The interior verdict, published by the classifier (`crate::interior`) — indoors the MCSH
    /// sample is overridden by the day/night target ([`DAYNIGHT_T`]).
    pub(crate) indoor: bool,
    /// Standing on an outdoor-class WMO surface (street/deck/porch — `MOGI & 0x48`), published by
    /// the classifier: the MCSH verdict of the terrain BENEATH the building is overridden by the
    /// lit target ([`LIT_T`]; the reference's own value here is intensity 2.5) — byte-verified
    /// (0477/0480):
    /// the down-ray attach's WMO branch sets the skip-shadow bit `[node+0xd]|=0x2` (`0x6a8bc7`,
    /// every node subclass), the terrain branch clears it (`0x6a8bed`), and the exterior intensity
    /// leg commits the constant 2.5 whenever it's set (`0x69e483`→`0x69e4ad` — the MCSH sample
    /// runs only terrain-linked). `self.target` keeps the raw sample so stepping off onto real
    /// terrain resumes from it.
    pub(crate) on_wmo: bool,
    /// The ramped ambient word (0..1 per channel) — the bake fold's ambient input, chasing
    /// [`Self::ambient_target`] at [`AMBIENT_RAMP_PER_SEC`]. Seeded by the classifier on bake
    /// entry (from the scene ambient, so walking into a warm room ramps rather than pops).
    pub(crate) ambient: Vec3,
    pub(crate) ambient_target: Vec3,
    /// The last effective target the `WOW_INTERIOR_LOG` instrument printed (log-on-change; starts
    /// off-scale so the first resolved target always prints).
    logged_target: f32,
    /// The shade byte the descendant walk last pushed to this root's parts — the settled gate
    /// (1490): while it matches this frame's byte and no other lane rewrote a part's tag (the
    /// reclaim detector in [`update_ground_shade`]), the walk is skipped whole. `None` = the
    /// parts are not known to carry any byte — fresh, or hidden by the election (a wake must
    /// re-assert even at an unchanged byte).
    asserted: Option<u8>,
}

impl Default for GroundShade {
    fn default() -> Self {
        Self {
            t: LIT_T,
            target: LIT_T,
            last_pos: Vec3::ZERO,
            sampled: false,
            indoor: false,
            on_wmo: false,
            ambient: Vec3::ZERO,
            ambient_target: Vec3::ZERO,
            logged_target: -1.0,
            asserted: None,
        }
    }
}

impl GroundShade {
    /// The node's current committed intensity (`[node+0xa4]`): 2.5 lit → 0.5 MCSH-shadowed, the
    /// day/night 1.0 at [`DAYNIGHT_T`] — the bake fold multiplies its diffuse word by this.
    pub(crate) fn intensity(&self) -> f32 {
        2.5 - 2.0 * self.t
    }

    /// The intensity chase's EFFECTIVE target: indoors the day/night point overrides the MCSH
    /// verdict; on an outdoor-class WMO surface the LIT point overrides (`self.target` keeps the raw
    /// sample, so a root stepping back onto terrain resumes from it); otherwise the MCSH verdict
    /// stands, for a unit exactly as for a GameObject — the target law is byte-shared and single-site
    /// (`69e4ad`/`69e496`) and we model the registered
    /// delivery that consumes it (0814).
    fn effective_target(&self) -> f32 {
        if self.indoor {
            DAYNIGHT_T
        } else if self.on_wmo {
            LIT_T
        } else {
            self.target
        }
    }

    /// Whether both chases sit on their targets — the classifier's refold gate (a Bake anchor
    /// keeps refolding its owned probe while either ramp moves). Compares against the EFFECTIVE
    /// intensity target: an indoor unit over MCSH-shadowed terrain (any unit inside a building —
    /// buildings bake their own footprint shadow) settles at the day/night point, and comparing
    /// the raw MCSH sample here kept every settled indoor Bake unit refolding forever.
    pub(crate) fn ramps_settled(&self) -> bool {
        (self.t - self.effective_target()).abs() < RAMP_EPS
            && (self.ambient - self.ambient_target).abs().max_element() < RAMP_EPS
    }

    /// Bake-entry seed (the classifier, on a lane change INTO the footprint bake): the ambient
    /// chase starts from the scene ambient the entity was just lit by, targeting the floor's
    /// cap-96 word — the reference's node carries its ramped `[+0x9c]` across the leg flip.
    pub(crate) fn seed_ambient(&mut self, from: Vec3, target: Vec3) {
        self.ambient = from;
        self.ambient_target = target;
    }
}

/// **This part's light is its own `CMapDoodadDef`'s, not any ancestor entity's light node** — a
/// WMO doodad prop spawned onto a streamed GameObject (a transport's deck cargo and its cabin
/// furniture, `entities::wmo_props`). This file's walk must pass it by whichever payload it is on.
///
/// **The reference proves the node cannot reach it**: a
/// transport's WMO is linked into the SAME global `TSExplicitList<CMapObjDef>` (`0xca7d98`) the
/// WMO scene walk iterates — there is no separate GameObject WMO band — so its MODD entries are
/// ordinary doodad defs. And `[def+0xa4]`, the sun scale this file's byte models, has exactly
/// FOUR writers image-wide: the ctor `0x6a7d73` (0.0), the WMO doodad-set commit `0x695bb3`
/// (1.0), the ADT MDDF commit `0x6b01bb` (1.0), and the MCSH refresh `0x698cb4` (0.5). None of
/// them is the light node: the 2.5-producer `0x69e280` is entered only from `0x671a73`/`0x69e913`
/// and the ramp `0x69e770` only through the thunk `0x671a90`, whose callers all load `[obj+0xe0]`
/// — a path a doodad def never takes. A transport has no M2 for a node to light at all
/// (`0x5f80e0` returns 0 for the transport types).
///
/// So the faithful value is **1.0**, or 0.5 from a SINGLE MCSH sample at the doodad's own
/// footprint — `0x698c50` is a one-shot queue drain that unlinks each entry, so the verdict
/// freezes at the pose the prop became resident at, and never ramps. Pushing the host's ramped,
/// re-sampled byte onto these parts was a divergence (decision 2047, the other half of 2031).
#[derive(Component)]
pub struct DoodadDefLit;

/// Roots owed a shade re-assert this frame even at an unchanged byte: some OTHER lane rewrote a
/// descendant part's `MeshTag` since [`detect_shade_reclaims`] last ran — the interior
/// classifier's exterior reclaim (1358), a late-attached part (`Added` counts as changed).
/// What lets [`update_ground_shade`]'s per-part walk skip the settled crowd (1490): steady
/// frames have no foreign tag writes, so this is empty and every settled drawn body costs the
/// ramp arithmetic and one map insert.
#[derive(Resource, Default)]
pub(crate) struct ShadeDirtyRoots(bevy::ecs::entity::EntityHashSet);

/// The reclaim detector — its own system because `Changed<MeshTag>` is read access and the
/// walk's query holds `&mut MeshTag` (B0001). Runs between the classifier and the walk, so a
/// same-frame reclaim is caught the frame it happens; a writer scheduled after the walk
/// (a PostUpdate fade) is caught on the next frame, exactly when the old unconditional walk
/// would have repaired it too. The walk's own writes land after this system's baseline and so
/// echo back as dirty for ONE extra frame — that echo walk compares every byte equal, writes
/// nothing, and the chain goes quiet; it cannot oscillate.
pub(crate) fn detect_shade_reclaims(
    changed_parts: Query<Entity, (Changed<MeshTag>, Without<crate::billboard::BillboardCard>)>,
    shade_roots: Query<(), With<GroundShade>>,
    child_of: Query<&ChildOf>,
    mut dirty: ResMut<ShadeDirtyRoots>,
) {
    dirty.0.clear();
    for part in &changed_parts {
        let mut node = part;
        loop {
            if shade_roots.contains(node) {
                dirty.0.insert(node);
                break;
            }
            let Ok(up) = child_of.get(node) else { break };
            node = up.parent();
        }
    }
}

/// `WOW_SHADE_CENSUS=<secs>` (any unparseable value = 5): a periodic one-line count of the two
/// populations this pass has to keep apart — every part carrying the doodad-def marker, and the
/// tagged parts *under* a shade root with the marked subset called out. B373 (2031/2041/2047) is
/// what made the pair worth counting: a transport's cabin prop sits under the boat's shade root
/// and must NOT take the boat's light, so "under a root" and "marked" diverging is the whole
/// diagnosis, and the two numbers moving together again is how a regression shows up. Zero-cost
/// when off: one env read, once.
fn shade_census_every() -> Option<f32> {
    static EVERY: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    *EVERY.get_or_init(|| {
        std::env::var("WOW_SHADE_CENSUS")
            .ok()
            .map(|v| v.parse().unwrap_or(5.0))
    })
}

/// The `shade-census` printer (`WOW_SHADE_CENSUS=<secs>`) — see [`shade_census_every`].
fn census_shade_marks(
    marked: Query<(), With<DoodadDefLit>>,
    roots: Query<Entity, With<GroundShade>>,
    children: Query<&Children>,
    tagged: Query<(), With<MeshTag>>,
    time: Res<Time>,
    mut next: Local<f32>,
) {
    let Some(every) = shade_census_every() else {
        return;
    };
    let now = time.elapsed_secs();
    if now < *next {
        return;
    }
    *next = now + every;
    let mut under_roots = 0usize;
    let mut marked_under_roots = 0usize;
    for r in &roots {
        for e in children.iter_descendants(r) {
            if tagged.get(e).is_ok() {
                under_roots += 1;
                if marked.get(e).is_ok() {
                    marked_under_roots += 1;
                }
            }
        }
    }
    info!(
        "shade-census: DoodadDefLit total {} | tagged parts under a shade root {under_roots}, of which marked {marked_under_roots}",
        marked.iter().count(),
    );
}

pub(crate) struct EntityShadePlugin;

impl Plugin for EntityShadePlugin {
    fn build(&self, app: &mut App) {
        // After the interior classifier: its exterior reclaim writes a fresh tag (shade byte 0) the
        // same frame this re-asserts the ramped byte, so the pair can't fight across frames. The
        // detector sits between the two so the reclaim is observed the frame it happens.
        app.init_resource::<ShadeDirtyRoots>().add_systems(
            Update,
            (
                detect_shade_reclaims,
                update_ground_shade,
                census_shade_marks,
            )
                .chain()
                .after(classify_entity_interior),
        );
    }
}

/// Sample + ramp each shaded root, then push the byte to its parts' tags (change-gated per part).
#[allow(clippy::type_complexity)]
// A Bevy system's params are not an argument list to shorten — each is a distinct world access the
// scheduler needs by name, and the card pass below deliberately takes its own disjoint `MeshTag`
// query rather than smuggling one through a shared `ParamSet`.
pub(crate) fn update_ground_shade(
    time: Res<Time>,
    streamer: Option<Res<TerrainStreamer>>,
    adt_tiles: Res<Assets<AdtTile>>,
    mut roots: Query<(
        Entity,
        &GlobalTransform,
        &mut GroundShade,
        Option<&crate::world_unit::ViewerUnit>,
        // The election's verdict on this root (1270/1475) — `Option` because a fixed doodad's
        // shade root carries no `Visibility` of its own and must keep walking.
        Option<&Visibility>,
    )>,
    children: Query<&Children>,
    // Parts are matched by carrying a `MeshTag`; a part whose payload is a PROBE SLOT is skipped,
    // because the shade byte lives in the same bits (see [`probe_payload`]). Fading parts are NOT
    // skipped — shade and fade own disjoint fields.
    mut parts: Query<
        (
            &mut MeshTag,
            Option<&InteriorLit>,
            Has<InteriorProbePayload>,
            Has<DoodadDefLit>,
        ),
        Without<crate::billboard::BillboardCard>,
    >,
    // A card is a world ROOT (the facing system owns its transform), so the descendant walk below
    // cannot reach one — it carries its owner instead. Disjoint from `parts` by the filter above.
    // It asks the same payload question: an interior prop's glow card carries its doodad's probe
    // slot, and this pass reaches it by walking UP from the card's owner (B373).
    mut cards: Query<(
        &crate::billboard::BillboardCard,
        &mut MeshTag,
        Option<&InteriorLit>,
        Has<InteriorProbePayload>,
        Has<DoodadDefLit>,
    )>,
    // Reused across frames: each shaded ROOT → its shade byte this frame (a few hundred entries).
    // The card pass walks `ChildOf` up to the nearest such root — this map used to record every
    // descendant too (~10-20k inserts/frame) so that walk could be a single lookup.
    mut root_shade: Local<bevy::ecs::entity::EntityHashMap<u8>>,
    // The card pass's up-walk: a card can follow a deep JOINT (`following_joint` — the swinging
    // lamp's glow), and a joint is not a shade root.
    child_of: Query<&ChildOf>,
    // The reclaim detector's verdict ([`detect_shade_reclaims`], one stage earlier): roots owed
    // a re-assert even at an unchanged byte.
    dirty_roots: Res<ShadeDirtyRoots>,
    mut self_log: Local<f32>,
) {
    // The map persists across frames (decision 1979): a settled root's byte is already in it,
    // and re-inserting ~1.3 k entries a frame was the walk's cost. A despawned root's entry is
    // dead weight nothing reads — entity ids are generational, so a reuse never aliases it.
    let Some(streamer) = streamer else {
        return;
    };
    let step = SHADE_RAMP_PER_SEC * time.delta_secs();
    let ambient_step = AMBIENT_RAMP_PER_SEC * time.delta_secs();
    for (root, gt, shade, is_self, root_vis) in &mut roots {
        // `WOW_INTERIOR_LOG=1`: a periodic SELF-node dump (every 3 s) — the probe run's definitive
        // "what state is the parked character actually in" line, attributable unlike the
        // change-triggered `[node]` lines below (wandering NPCs fire those constantly).
        if is_self.is_some() && time.elapsed_secs() - *self_log > 3.0 && interior_log_enabled() {
            let p = gt.translation();
            eprintln!(
                "[self-node] at wow ({:.1}, {:.1}, {:.1})  t {:.2} -> {:.2} (I {:.2})  indoor {} \
                 on_wmo {}",
                -p.z,
                -p.x,
                p.y,
                shade.t,
                shade.effective_target(),
                2.5 - 2.0 * shade.t,
                shade.indoor,
                shade.on_wmo,
            );
            *self_log = time.elapsed_secs();
        }
        let shade = shade.into_inner(); // one deref; field writes below are unconditional-cheap
        let pos = gt.translation();
        // Re-sample the MCSH bit only on real movement (or the very first pass) — the global
        // world→tile→chunk lookup is cheap, but a town of standing NPCs shouldn't run it per frame.
        if !shade.sampled || pos.distance_squared(shade.last_pos) >= RESAMPLE_DIST_SQ {
            match doodad_ground_shade(&streamer, &adt_tiles, pos) {
                ShadeResolve::Ready(shadowed) => {
                    shade.target = if shadowed { 1.0 } else { LIT_T };
                    shade.last_pos = pos;
                    if !shade.sampled {
                        // First landing: snap — an entity spawns already at its ground's shade
                        // (the appear-fade covers the arrival; a ramp-in from lit would read as a
                        // lighting pop right after materializing).
                        shade.t = shade.effective_target();
                        shade.sampled = true;
                    }
                }
                // The tile under the entity is requested but still decoding — keep the last state
                // and retry next frame (mirrors the doodad spawn's deferral).
                ShadeResolve::Pending => {}
            }
        }
        // Indoors the MCSH sample is moot: the day/night intensity target is 1.0 (the reference's
        // interior `[+0xf8]`, decision 0354). The sample above still ran its movement gate, so
        // stepping back outside resumes from a fresh MCSH verdict.
        let target = shade.effective_target();
        // `WOW_INTERIOR_LOG=1`: one line whenever a node's intensity target moves — the live
        // instrument for "which stage is this character actually in?" — MCSH-shadowed 0.5 ⇒ t 1;
        // exterior lit and day/night both ⇒ t 0.75 / committed 1.0 while the gain cap stands (0821).
        if (target - shade.logged_target).abs() > f32::EPSILON && interior_log_enabled() {
            eprintln!(
                "[node] root {root:?} at ({:.1}, {:.1}, {:.1}) -> target t {target:.2} \
                 (I {:.2}, indoor {}) from t {:.2}",
                pos.x,
                pos.y,
                pos.z,
                2.5 - 2.0 * target,
                shade.indoor,
                shade.t,
            );
            shade.logged_target = target;
        }
        // The reference ramps: linear toward the target, never past it — intensity (as the mix
        // `t`) at 3.3333/s over its span, the ambient word at 2.0/s per channel (`0x69e770`).
        shade.t = if shade.t < target {
            (shade.t + step).min(target)
        } else {
            (shade.t - step).max(target)
        };
        let a = shade.ambient;
        let at = shade.ambient_target;
        shade.ambient = Vec3::new(
            ramp_toward(a.x, at.x, ambient_step),
            ramp_toward(a.y, at.y, ambient_step),
            ramp_toward(a.z, at.z, ambient_step),
        );
        let byte = (shade.t * 255.0).round().clamp(0.0, 255.0) as u8;
        // Every root enters the card map, hidden or not — the insert is one hash write, and a
        // hidden body's card used to walk its whole parent chain to a `None` precisely because
        // its root was left out.
        if root_shade.get(&root) != Some(&byte) {
            root_shade.insert(root, byte);
        }
        // A root the election hid (1270/1475) draws nothing, so the re-assert walk below is
        // skipped whole — 1473's audit found it running over every part of every off-view body.
        // The ramps above kept stepping, so the byte is current the frame the body wakes; the
        // cleared `asserted` is what makes the wake frame's walk unconditional, into parts
        // whose tags retained their last byte.
        if root_vis.is_some_and(|v| *v == Visibility::Hidden) {
            shade.asserted = None;
            continue;
        }
        // The settled gate (1490): every part below already carries this byte (we pushed it —
        // `asserted`) and no other lane rewrote a tag since (`dirty_roots`). The walk used to be
        // unconditional for a drawn root as the MeshTag re-assert over the classifier's
        // exterior reclaim (1358); the reclaim detector above answers that by observation
        // instead of by re-walking every body every frame.
        if shade.asserted == Some(byte) && !dirty_roots.0.contains(&root) {
            continue;
        }
        shade.asserted = Some(byte);
        // Push to every part below the root (body submeshes are direct children; held items/helm ride
        // joint entities deeper down — same full-tree walk as the self-fade). Change-gated per part on
        // the byte, so a settled entity writes nothing and never re-triggers render extraction.
        for part in children.iter_descendants(root) {
            let Ok((mut tag, lit, own_probe, own_def)) = parts.get_mut(part) else {
                continue;
            };
            // Two reasons this walk passes a part by, and they are different questions.
            // OWNERSHIP: a WMO doodad prop is lit by its own def, never by the node above it
            // (`DoodadDefLit`) — it is only in this tree so it rides a moving transport.
            if own_def {
                continue;
            }
            // PAYLOAD: asked the only way the shade accessors can be reached
            // (`mesh_tag::exterior_payload`); `None` = this part's bits 6..=18 are a probe slot.
            let Some(ext) = exterior_payload(lit.is_some_and(InteriorLit::is_bake), own_probe)
            else {
                continue;
            };
            if shade_of(tag.0, ext) != byte {
                tag.0 = with_shade(tag.0, byte, ext);
            }
        }
    }
    // The cards (0788's loose end). A card belongs to the same light node as the body it hangs off —
    // the reference shades every batch of an object through one node (0778) — but it is a world root,
    // so the walk above skips it and it kept the lit rung while its owner dimmed. 0811 scoped this to
    // GameObjects because 0809 had pinned units flat; 0814 put units back on the chase, so it is once
    // again every carried card — a torch's flame card dims with the hand that holds it. The owner (an
    // anchor or a deep joint) resolves to the NEAREST shaded root above it: with nested roots (a
    // mounted unit — rider and mount each carry a node) that is the mount's, matching the
    // one-node-per-object structure above; the old whole-tree map made this pick last-writer-wins.
    for (card, mut tag, lit, own_probe, own_def) in &mut cards {
        // Both questions again, and a card needs them at least as much: it is a world ROOT, so
        // this pass finds its owner by walking UP — which is how a deck lantern's glow card and a
        // cabin prop's alike reached the host GameObject that does not light them (B373, 2047).
        if own_def {
            continue;
        }
        let Some(ext) = exterior_payload(lit.is_some_and(InteriorLit::is_bake), own_probe) else {
            continue;
        };
        let Some(byte) = card_root_shade(&root_shade, &child_of, card.follows()) else {
            continue; // a fixed terrain doodad's card — its shade rides the material selector
        };
        if shade_of(tag.0, ext) != byte {
            tag.0 = with_shade(tag.0, byte, ext);
        }
    }
}

/// The shade byte a card inherits: the NEAREST shaded root at or above `follows` (the owner itself
/// first, then the `ChildOf` chain). `None` — no owner, or no shaded root over it — skips the card,
/// exactly as the old whole-tree map's miss did.
fn card_root_shade(
    root_shade: &bevy::ecs::entity::EntityHashMap<u8>,
    child_of: &Query<&ChildOf>,
    follows: Option<Entity>,
) -> Option<u8> {
    let mut node = follows?;
    loop {
        if let Some(&byte) = root_shade.get(&node) {
            return Some(byte);
        }
        node = child_of.get(node).ok()?.parent();
    }
}

/// `WOW_INTERIOR_LOG=1` — the interior/shade instrument lines. Resolved once: the raw env read ran
/// per root per frame.
fn interior_log_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_INTERIOR_LOG").is_some())
}

/// One linear ramp step toward a target, never past it (the binary's clamp-no-overshoot chase).
fn ramp_toward(v: f32, target: f32, step: f32) -> f32 {
    if v < target {
        (v + step).min(target)
    } else {
        (v - step).max(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::entity::EntityHashMap;
    use bevy::ecs::system::SystemState;

    /// Run the card resolve against a world's live `ChildOf` topology.
    fn resolve(world: &mut World, map: &EntityHashMap<u8>, follows: Option<Entity>) -> Option<u8> {
        let mut state: SystemState<Query<&ChildOf>> = SystemState::new(world);
        card_root_shade(map, &state.get(world), follows)
    }

    /// The reclaim detector's tick contract (1490): a fresh part flags its root (Added counts
    /// as changed); a quiet frame flags nothing; a foreign tag write flags the root again — and
    /// only the root above the touched part, not every root in the world. This is what lets
    /// the shade walk skip the settled crowd without ever missing 1358's exterior reclaim.
    #[test]
    fn a_foreign_tag_write_flags_exactly_its_root() {
        let mut app = App::new();
        app.init_resource::<ShadeDirtyRoots>()
            .add_systems(Update, detect_shade_reclaims);
        let root = app.world_mut().spawn(GroundShade::default()).id();
        let part = app.world_mut().spawn((MeshTag(7), ChildOf(root))).id();
        let other = app.world_mut().spawn(GroundShade::default()).id();
        app.world_mut().spawn((MeshTag(9), ChildOf(other)));

        app.update(); // both parts are Added ⇒ both roots flagged
        let dirty = |app: &App| {
            let d = app.world().resource::<ShadeDirtyRoots>();
            (d.0.contains(&root), d.0.contains(&other))
        };
        assert_eq!(dirty(&app), (true, true), "a fresh part flags its root");

        app.update(); // nobody wrote ⇒ quiet
        assert_eq!(dirty(&app), (false, false), "a settled frame flags nothing");

        app.world_mut().get_mut::<MeshTag>(part).unwrap().0 = 42;
        app.update();
        assert_eq!(
            dirty(&app),
            (true, false),
            "a foreign write flags its own root and nobody else's"
        );
    }

    /// A card's owner can sit arbitrarily deep in its root's tree (a held torch's flame card
    /// follows a hand JOINT under an item entity under the unit) — the up-walk finds the root's
    /// byte; the root itself, as an owner, is the walk's zero-hop case.
    #[test]
    fn a_card_takes_its_roots_byte_from_any_depth() {
        let mut world = World::new();
        let root = world.spawn_empty().id();
        let a = world.spawn(ChildOf(root)).id();
        let b = world.spawn(ChildOf(a)).id();
        let c = world.spawn(ChildOf(b)).id();
        let mut map = EntityHashMap::default();
        map.insert(root, 7);
        assert_eq!(resolve(&mut world, &map, Some(c)), Some(7), "3 deep");
        assert_eq!(
            resolve(&mut world, &map, Some(root)),
            Some(7),
            "the root itself"
        );
    }

    /// No owner (a fixed terrain doodad's card), and an owner whose ancestor chain holds no shaded
    /// root (a card of something this system doesn't shade) — both skip, like the old map miss.
    #[test]
    fn an_unshaded_card_is_skipped() {
        let mut world = World::new();
        let stray_root = world.spawn_empty().id();
        let stray = world.spawn(ChildOf(stray_root)).id();
        let mut map = EntityHashMap::default();
        map.insert(world.spawn_empty().id(), 9);
        assert_eq!(resolve(&mut world, &map, None), None, "follows nothing");
        assert_eq!(
            resolve(&mut world, &map, Some(stray)),
            None,
            "no shaded ancestor"
        );
    }

    /// **The payload question, both populations** (B373). The guard used to ask "is this part on
    /// the classifier's Bake law", which is only one of the two ways a `MeshTag` comes to hold a
    /// probe slot; a WMO doodad prop holds one from spawn and carries no `InteriorLit` at all.
    /// A transport's cabin furniture is where the second population lands inside an entity's
    /// descendant walk, and every one of those props drew under a foreign probe.
    ///
    /// The question now lives in `mesh_tag` beside the bits it is about, and the shade accessors
    /// take its answer as a witness — so this test is about the two populations, not about a
    /// caller remembering to ask, which the type system now handles.
    #[test]
    fn either_probe_population_denies_the_shade_writer_its_witness() {
        assert!(
            exterior_payload(false, false).is_some(),
            "an ordinary exterior part takes the shade byte"
        );
        assert!(
            exterior_payload(true, false).is_none(),
            "the classifier's Bake law owns the payload"
        );
        assert!(
            exterior_payload(false, true).is_none(),
            "a spawn-time MODD prop owns it too, with no InteriorLit to say so"
        );
        assert!(
            exterior_payload(true, true).is_none(),
            "and either alone is enough — the question is an OR, not a tie-break"
        );
    }

    /// Nested shade roots — a mounted unit: rider root and mount root each carry a node, the mount
    /// a descendant of the rider. A card under the MOUNT takes the mount's byte (the nearest root),
    /// pinned here because the old whole-tree map answered this by insert order.
    #[test]
    fn a_nested_root_wins_by_nearness() {
        let mut world = World::new();
        let rider = world.spawn_empty().id();
        let mount = world.spawn(ChildOf(rider)).id();
        let mount_joint = world.spawn(ChildOf(mount)).id();
        let mut map = EntityHashMap::default();
        map.insert(rider, 3);
        map.insert(mount, 9);
        assert_eq!(
            resolve(&mut world, &map, Some(mount_joint)),
            Some(9),
            "the MOUNT's byte"
        );
        assert_eq!(resolve(&mut world, &map, Some(mount)), Some(9));
    }
}
