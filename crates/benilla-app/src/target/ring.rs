//! The ground selection ring — a projected decal under the current target, reaction-coloured.
//!
//! **Geometry: the reference's own mechanism** ([`project_ring`] — the collector `0x6723b0` is
//! terrain + WMO, flags `0x200122`, no walkability test): the actual
//! surface triangles inside the ring's box — terrain tiles + WMO faces
//! ([`GroundDecalSurface`]), **never** doodads/GameObjects — clipped to the box and textured
//! top-down, so the ring is pixel-coplanar with the visible ground, drapes down steps/ledges like
//! the reference, and passes *under* props. (A Bevy `ForwardDecal` was rejected: it distorts at
//! WoW's steep camera angle and its depth-prepass broke clutter. An earlier per-vertex height-probe
//! grid was replaced by this — a height field can't represent ledge faces and spiked on them.)
//!
//! **Texture + blend**: the reference's own `UnitSelectTexture.blp`, additive, **not** pulsed (the
//! reference's pulsing circle is a separate spell/AoE indicator). The baked alpha fade (bright arc,
//! fading tail) is oriented **camera-relative** each frame ([`ring_fade_angle`]): the bright arc
//! faces the viewer, the fade points away — the reference decal's behaviour (its projector
//! transform is camera-fed).
//!
//! **Colour: the ring's own selector** (`CGUnit::GetSelectionCircleColor` `0x605960`, trace + byte
//! verified — NOT the nameplate palette): players branch to pale blue / hostile red; NPCs to
//! dead-gray or the reaction-rank palette (red / orange / yellow / green) — see
//! [`RingMaterials::pick`]. The reaction rank resolves in the client's own order
//! ([`ring_reaction`]): **reputation rank first** (a reputation faction's NPCs colour by our
//! standing with them), else the faction-template comparator
//! ([`benilla_formats::FactionTemplate::reaction_toward`], the byte-exact `0x606640`), evaluated as
//! the **unit's** reaction toward the local player (byte-verified direction). Death **clears the
//! target** on the alive→dead transition — the reference's own mechanism: the health mirror's
//! death edge fires the CGUnit death handler
//! (`0x605860`), which clears a matching selection and sends `CMSG_SET_SELECTION 0`.
//!
//! Still *interim*: the vertical fade profile on stretched (wall/ledge) pieces is capture-matched —
//! the reference's edge-fade grid (`0x6147f0`) is byte-located but its ramp is underived (an open
//! question). The ring appears at full brightness the instant of selection — director-verified and
//! byte-confirmed (the retracted "2 s selection fade-in" note was a misread of the *scale-change*
//! easing, see [`crate::net::NetEntity`]; no fade arms on any selection path). The selector's
//! **first-priority branch** — the melee combat flash (`[unit+0xc58]` bit 0x10, the red↔orange
//! triangle pulse) — is live: [`super::CombatFlash`] carries the frame's verdict + colour and
//! outranks every branch below (player/dead/reaction), the byte order. The player path's party
//! legs are live (0434 phase 6 — pale blue / pale green off the roster). Deferred selector
//! states: the full PvP attackability matrix (X/Y), forced reactions, contested-guard.

use benilla_formats::{load_faction_catalog, reputation_rank, FactionCatalog, Reaction};
use benilla_protocol::EntityKind;
use bevy::prelude::*;

use crate::net::{Guid, NetEntity, ObjectStore, Reputations, SelfPlayer};
use benilla_assets::{LockRecover, WorldAssets};
use benilla_world::decal::{DecalFrame, WorldDecal};
use benilla_world::particles::buffer::EffectVertex;

use super::click::clear;
use super::{CombatFlash, Selection, SelectionRadius};
use crate::creature_anim::Engaged;
use benilla_world::view::WorldCamera;

/// The single ring record (a resource since 0733 — the projection rides the effect stream; no
/// entity, no mesh, no materials). `update_ring` rebuilds `verts` when the [`RingKey`] moves and
/// re-resolves `color` every frame; `push_ring` copies the slice onto the stream tinted.
#[derive(Resource, Default)]
pub(super) struct RingState {
    verts: Vec<EffectVertex>,
    key: RingKey,
    /// This frame's resolved tint (the selector's dword — or the combat flash's wave sample).
    color: Color,
    shown: bool,
}

/// The projection's rebuild inputs — a still target under a still camera costs a compare
/// (the ShadowKey treatment; the old path re-projected every shown frame).
#[derive(Default, PartialEq, Clone, Copy)]
struct RingKey {
    feet: Vec3,
    radius: f32,
    fade_angle: f32,
    surfaces: usize,
}

/// The ring texture (the render-side residency gate withholds the draw until it loads).
#[derive(Resource)]
pub(super) struct RingAssets {
    texture: Handle<Image>,
}

/// The reference's ground selection-circle texture (`Textures\UnitSelectTexture.blp`, loaded at
/// `0x6146d0`) — a white ring, sampled top-down, tinted + additively blended below.
const RING_TEXTURE: &str = "mpq://textures/unitselecttexture.blp";
/// Model-local ring radius for a unit with **no model at all** (a cube fallback), since it has no M2
/// footprint to measure. The reference's own degenerate-box constant rather than a
/// number of ours: a body whose box measures zero and a body with no box are the same question, and
/// `0x60aee0` answers it with 1.2.
const RING_FALLBACK_RADIUS: f32 = benilla_formats::DEGENERATE_RING_FOOTPRINT;
/// The ring's own palette (the
/// `CGUnit::GetSelectionCircleColor` selector `0x605960`, per-object vtable `+0x2c`; the dword is
/// written verbatim as every decal vertex's diffuse, alpha 255 — no tint global, no tex-env
/// constant). The ring does **not** use the nameplate palette: player-blue is the pale
/// **`0xFF6060FF`** (96,96,255), not the nameplate's pure blue. NPC branch indexes the raw reaction
/// rank: 0–1 red `0xFFFF0000`, **2 unfriendly orange `0xFFFF8000`**, 3 neutral yellow `0xFFFFFF00`,
/// 4–7 friendly green `0xFF00FF00`; a **dead NPC** overrides to mid-gray `0xFF7F7F7F` (players skip
/// the health check). Tints draw at full strength (an earlier ×0.5 theory was refuted by pixels).
/// The selector's first-priority branch — the combat flash (`0xFFFF0000↔0xFFFF8000` pulse) — is
/// live via [`super::CombatFlash`] and outranks all of these. The player path's `¬X∧¬Y` leg is
/// live (0434 phase 6): PvP-flagged → green `0xFF00FF00` (party member → pale-green
/// `0xFFAAFFAA`), unflagged → the soft blue (party member → pale-blue `0xFFAAAAFF`, the
/// selector's 4-slot party-guid table `0xbc6f48` = our roster). Still deferred: the
/// cross-faction attackability matrix X/Y (approximated as hostile-red on rank ≤ 1) and the
/// CHARMEDBY/SUMMONEDBY owner resolve inside the PvP-flag read (we read the unit's own flag).
// GAMMA LANE: raw authored bytes into the gamma framebuffer (see nameplates.rs).
const RING_HOSTILE: Color = Color::linear_rgb(1.0, 0.0, 0.0);
const RING_UNFRIENDLY: Color = Color::linear_rgb(1.0, 0.502, 0.0);
const RING_NEUTRAL: Color = Color::linear_rgb(1.0, 1.0, 0.0);
const RING_FRIENDLY: Color = Color::linear_rgb(0.0, 1.0, 0.0);
const RING_PLAYER: Color = Color::linear_rgb(0.376, 0.376, 1.0);
const RING_DEAD: Color = Color::linear_rgb(0.498, 0.498, 0.498);
const RING_PARTY: Color = Color::linear_rgb(0.667, 0.667, 1.0); // 0xFFAAAAFF
const RING_PARTY_PVP: Color = Color::linear_rgb(0.667, 1.0, 0.667); // 0xFFAAFFAA

/// The FactionTemplate.dbc catalog, for the ring's reaction colour. Absent if the DBC failed to load
/// (the ring then stays the neutral fallback).
#[derive(Resource)]
pub(crate) struct Factions(FactionCatalog);

impl Factions {
    /// The loaded catalog — for sibling faction consumers (the zone PvP state reads our own
    /// template's group mask through it).
    pub(crate) fn catalog(&self) -> &FactionCatalog {
        &self.0
    }

    /// Wrap a catalog — **tests only**. A sibling module exercising a reaction gate needs a real
    /// `Factions` in its world, and the field stays private otherwise: in a running client the
    /// resource is built exactly once, from the DBC, by the loader below.
    #[cfg(test)]
    pub(crate) fn from_catalog(catalog: FactionCatalog) -> Self {
        Self(catalog)
    }
}

/// The colour `GetSelectionCircleColor` resolves — the pure classification half of the
/// selector, split out so the branch logic (players vs NPCs, the `¬X∧¬Y` party split) is
/// unit-testable.
///
/// **This is the single source for BOTH surfaces the selector feeds** — the ground ring here and
/// the overhead name (`nameplates.rs` fetches the same `vtable+0x2c`). It was a
/// duplicated mirror until decision 0659: the ring gained the PvP/party legs with 0453 and the
/// name's copy did not, so a flagged player drew a green ring under a blue name. One law, one
/// function; the name maps this to its own material cache.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum RingVariant {
    Hostile,
    Unfriendly,
    Neutral,
    Friendly,
    Player,
    Dead,
    Party,
    PartyPvp,
}

impl RingVariant {
    /// Every variant, for a consumer that pre-builds one material per colour.
    pub(crate) const ALL: [Self; 8] = [
        Self::Hostile,
        Self::Unfriendly,
        Self::Neutral,
        Self::Friendly,
        Self::Player,
        Self::Dead,
        Self::Party,
        Self::PartyPvp,
    ];

    /// The selector's dword for this variant (see the palette constants above).
    pub(crate) fn color(self) -> Color {
        match self {
            Self::Hostile => RING_HOSTILE,
            Self::Unfriendly => RING_UNFRIENDLY,
            Self::Neutral => RING_NEUTRAL,
            Self::Friendly => RING_FRIENDLY,
            Self::Player => RING_PLAYER,
            Self::Dead => RING_DEAD,
            Self::Party => RING_PARTY,
            Self::PartyPvp => RING_PARTY_PVP,
        }
    }
}

/// The ring's own colour selector (`0x605960`) on the raw reaction rank (`0..=7`): **players**
/// branch first and never check health (a dead player doesn't gray) — hostile red on rank ≤ 1
/// (the X/Y matrix approximation), else the `¬X∧¬Y` split: PvP-flagged → green / party
/// pale-green, unflagged → soft blue / party pale-blue (the 4-slot party table `0xbc6f48`, our
/// roster — self is never in it, and self reads blue/green exactly like the law's own-guid legs);
/// **NPCs** — dead → gray, else the rank palette (0–1 red, 2 orange, 3 yellow, 4–7 green).
///
/// Shared with the overhead name — see [`RingVariant`].
pub(crate) fn ring_variant(
    rank: u8,
    is_player: bool,
    is_dead: bool,
    pvp: bool,
    in_party: bool,
) -> RingVariant {
    if is_player {
        return if rank <= 1 {
            RingVariant::Hostile
        } else if pvp {
            if in_party {
                RingVariant::PartyPvp
            } else {
                RingVariant::Friendly
            }
        } else if in_party {
            RingVariant::Party
        } else {
            RingVariant::Player
        };
    }
    if is_dead {
        return RingVariant::Dead;
    }
    match rank {
        0..=1 => RingVariant::Hostile,
        2 => RingVariant::Unfriendly,
        3 => RingVariant::Neutral,
        _ => RingVariant::Friendly,
    }
}

/// Load the ring texture and seed the ring record. (The old path built 9 tinted
/// `StandardMaterial` clones and a mesh asset here; the tint is per-vertex colour at push time
/// now — the reference writes the selector's dword as every decal vertex's diffuse, which is
/// exactly what the stream does.)
pub(super) fn setup_ring(mut commands: Commands, asset_server: Res<AssetServer>) {
    let texture = asset_server.load::<Image>(RING_TEXTURE);
    commands.insert_resource(RingAssets { texture });
    commands.init_resource::<RingState>();
}

/// Startup (after the MPQ chain opens): load FactionTemplate.dbc for the reaction colour. On failure
/// the resource is simply absent and the ring stays neutral yellow.
pub(super) fn load_factions(mut commands: Commands, world_assets: Option<Res<WorldAssets>>) {
    let Some(world_assets) = world_assets else {
        return;
    };
    let mut chain = world_assets.chain.lock_recover();
    match load_faction_catalog(&mut chain) {
        Ok(catalog) => {
            info!("faction catalog: {} template rows", catalog.len());
            commands.insert_resource(Factions(catalog));
        }
        Err(e) => warn!("faction catalog unavailable, ring stays neutral: {e:#}"),
    }
}

/// Position, size, colour + show the ring under the current target each frame; hide it when nothing is
/// selected. Radius = the unit's model ring footprint ([`SelectionRadius`], the Stand-box
/// `sqrt(0.5·sqrt(dx²+dy²))`, or 1.2 for a degenerate box) × its transform scale
/// (`OBJECT_FIELD_SCALE_X`). Colour = the target's
/// reaction rank ([`ring_reaction`]), re-resolved each frame (faction can change live — the store
/// merges `Values` deltas), the handle swapped only on change. No pulse — the reference's unit ring
/// is steady. If the target is no longer an object (destroyed / streamed out — torn down, even while
/// its model fades) the selection clears and the server is told — the reference's teardown clear
/// sends `CMSG_SET_SELECTION 0` on both paths.
#[allow(clippy::type_complexity)]
pub(super) fn update_ring(
    mut selection: ResMut<Selection>,
    factions: Option<Res<Factions>>,
    reputations: Res<Reputations>,
    flash: Res<CombatFlash>,
    mut state: ResMut<RingState>,
    camera: Query<&GlobalTransform, With<WorldCamera>>,
    decals: WorldDecal,
    // The last camera-relative fade angle, kept across frames so a degenerate (straight-down) camera
    // holds the previous orientation instead of snapping.
    mut fade_angle: Local<f32>,
    // The last guid whose colour decision was logged (log once per target change, not per frame).
    mut logged_guid: Local<Option<u64>>,
    // Last frame's (target guid, dead?) — the alive→dead edge on the *same* unit clears the
    // selection. Tracked per frame keyed on the guid, never armed only at selection *change*: a
    // `.respawn`ed creature reuses its spawn guid, so change-armed state goes stale and misses the
    // second kill (the bug this replaces).
    mut last_vitals: Local<Option<(u64, bool)>>,
    mut seam: crate::creature_anim::AttackSeam,
    // Net entities are roots, so their `Transform` is already world-space + current this frame
    // (net motion ran in `WorldStage::Net`), avoiding the 1-frame lag a `GlobalTransform` read
    // would add. Tupled into one param (the 16-param ceiling): `.0` the target's own components;
    // `.1` the mounted footprint source — while a mount model is attached, the ring reads the
    // MOUNT's Stand-box footprint at the mount's rendered scale (the `+0xcf0` ring cache
    // recomputes from the mount model's Stand
    // box, `0x60ce70` tail → `0x60aee0`; the rendered scale is
    // `SCALE_X × CreatureDisplayInfo.creatureModelScale`, and the child's `NetEntity.scale`
    // carries exactly the CDI column).
    //
    // `.0` is filtered on [`Guid`] — **a live object**, not merely a drawn model: a torn-down
    // unit keeps its model (and its `Transform`) for the two-second fadeout, but sheds its guid
    // at the teardown ([`crate::net::tear_down`]), so it lands in the gone-object branch below on
    // the teardown's own frame rather than when the fade despawns it.
    targets: (
        Query<
            (
                &Transform,
                Option<&SelectionRadius>,
                Option<&ObjectStore>,
                Option<&NetEntity>,
                Option<&crate::entities::mount::MountChild>,
            ),
            With<Guid>,
        >,
        Query<(&NetEntity, Option<&SelectionRadius>), With<crate::entities::mount::MountBody>>,
        // The party roster — the selector's 4-slot guid table (the party ring colours).
        Res<crate::ui_party::GroupState>,
    ),
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    // Are *we* mid auto-attack (server-echoed [`Engaged`])? Both clear paths below end the swing
    // then — the reference's death/teardown edges stop the attack along with the selection.
    engaged: Query<(), (With<Engaged>, With<SelfPlayer>)>,
) {
    let state = &mut *state;
    let hide = match selection.target {
        None => {
            // No target: drop the per-target trackers, so re-selecting the *same* guid later is a
            // fresh start (vitals re-read, colour decision re-logged).
            *last_vitals = None;
            *logged_guid = None;
            true
        }
        Some(target) => match targets.0.get(target) {
            Ok((unit, sel_radius, store, net, mount_child)) => {
                // A real model uses its own footprint, whatever it measures; only a model-less unit
                // falls back. **No floor** — the reference has none, and the one we used to keep
                // (0.05) is what a zero-bounds model landed on: `ring_footprint` now carries the
                // writer's own degenerate-box answer (1.2) instead of a 0 for the picker to clamp,
                // so the Naxxramas weapon mobs ring at 1.2 × 2.25 ≈ 2.7 yd like the reference and
                // not at a coin's width.
                // Mounted, the footprint and the extra scale column come from the mount child
                // (the `mount_parts` doc above); a still-loading mount rides the fallback the
                // way any model-less unit does until its bounds land.
                let (local, mount_scale) = match mount_child.and_then(|mc| targets.1.get(mc.0).ok())
                {
                    Some((mnet, msel)) => (msel.map_or(RING_FALLBACK_RADIUS, |r| r.0), mnet.scale),
                    None => (sel_radius.map_or(RING_FALLBACK_RADIUS, |r| r.0), 1.0),
                };
                let radius = local * (unit.scale.x * mount_scale).max(0.01);
                *fade_angle = ring_fade_angle(&camera).unwrap_or(*fade_angle);
                // The rebuild gate: a still target under a still camera keeps the
                // cached projection — the old path re-projected every shown frame.
                let key = RingKey {
                    feet: unit.translation,
                    radius,
                    fade_angle: *fade_angle,
                    surfaces: decals.receiver_count(),
                };
                let projected = if state.shown && key == state.key {
                    !state.verts.is_empty()
                } else {
                    state.verts.clear();
                    state.key = key;
                    project_ring(&mut state.verts, &decals, unit.translation, radius, {
                        *fade_angle
                    })
                };
                let rank = ring_reaction(
                    factions.as_deref(),
                    &reputations,
                    store,
                    self_store.single().ok(),
                );
                let is_player = net.is_some_and(|n| n.kind == EntityKind::Player);
                // The ¬X∧¬Y split's inputs: the unit's own PvP flag (UNIT_FIELD_FLAGS 0x1000;
                // the charm-owner resolve is noted residue) + roster membership by guid.
                let pvp = store.is_some_and(|s| s.0.unit_flags() & 0x1000 != 0);
                let in_party = selection
                    .guid
                    .is_some_and(|g| targets.2.members.iter().any(|m| m.guid == g));
                // Dead (health 0, absent-counts-as-zero — `unit_is_dead`) — re-read each frame, so
                // the ring reacts the moment the kill's health delta merges into the store, and an
                // already-dead corpse reads dead on stream-in.
                let is_dead = store.is_some_and(|s| s.0.unit_is_dead());
                // Death clears the target on the alive→dead *transition* — the reference's own
                // mechanism: the health mirror-
                // handler's death edge (`0x6046f0`, new ≤ 0 while old > 0) fires the CGUnit death
                // handler `0x605860`, which clears a matching selection via SetSelection(0) and
                // sends `CMSG_SET_SELECTION 0` — exactly what `clear` does. Edge-only, like the
                // reference: deliberately selecting a corpse afterwards stays valid (gray ring).
                // (The same edge also stops auto-attack and cancels a cast at the dying unit —
                // faithful siblings for when combat/casting exist.)
                let died = is_dead
                    && selection
                        .guid
                        .is_some_and(|g| *last_vitals == Some((g, false)));
                *last_vitals = selection.guid.map(|g| (g, is_dead));
                if died {
                    clear(&mut selection, &mut seam, !engaged.is_empty());
                    *last_vitals = None;
                    state.shown = false;
                    state.verts.clear();
                    return;
                }
                // One line per target change — the whole colour decision, so a wrong ring colour in
                // the field is diagnosable from the log instead of guessed at.
                if selection.guid != *logged_guid {
                    *logged_guid = selection.guid;
                    info!(
                        "target: guid {:?} ftpl {:?} (self ftpl {:?}) → rank {rank}{}{}",
                        selection.guid,
                        store.and_then(|s| s.0.unit_faction_template()),
                        self_store
                            .single()
                            .ok()
                            .and_then(|s| s.0.unit_faction_template()),
                        if is_player { " [player→blue]" } else { "" },
                        if is_dead { " [dead→gray]" } else { "" },
                    );
                }
                // The selector's first-priority branch (`0x605960`, byte order): the combat
                // flash outranks player/dead/reaction. The tint is a per-vertex colour at push
                // time now, so the flash's per-frame wave sample costs a resource write — the
                // old path's one material mutation is gone.
                state.color = if flash.unit == Some(target) {
                    flash.color
                } else {
                    ring_variant(rank, is_player, is_dead, pvp, in_party).color()
                };
                // No receiving surface in the box (mid-air, unstreamed tile) → hide, the reference's
                // own no-ground gate (`0x6d74b5`: the whole draw is skipped).
                !projected
            }
            // The target is no longer an object (destroyed or streamed out — torn down, its model
            // perhaps still fading; or despawned outright): clear, informing the server — the
            // reference's teardown does exactly this for both removal paths (object deactivate →
            // the selection clear + `CMSG_SET_SELECTION 0`, `0x5fbb60` → `0x493910`; this is also
            // what drops a selected corpse at respawn, when
            // the server destroys it ahead of the fresh create).
            Err(_) => {
                clear(&mut selection, &mut seam, !engaged.is_empty());
                *last_vitals = None;
                *logged_guid = None;
                true
            }
        },
    };
    state.shown = !hide;
    if hide {
        state.verts.clear();
    }
}

/// The ring fade's camera-relative angle θ: the texture's **faded side always points away from the
/// camera** (the bright arc faces the viewer — the reference decal's behaviour, director-confirmed;
/// its projector transform is camera-fed). The fade is baked into `UnitSelectTexture.blp`'s alpha
/// (bright at v=1 → ring-local **+Z**, transparent at v=0 → **−Z**, measured off the shipped
/// texture); [`project_ring`] rotates each vertex's UV by −θ, which is equivalent to yawing the
/// texture square by θ so its −Z side lands on the camera's ground-projected forward
/// (a yaw θ maps a local `atan2(z,x)` angle φ to `φ − θ`; local −Z sits at −π/2; so
/// `θ = −π/2 − atan2(f.z, f.x)`). `None` looking straight down (degenerate forward) — the caller
/// keeps the previous angle rather than snapping to an arbitrary one.
fn ring_fade_angle(camera: &Query<&GlobalTransform, With<WorldCamera>>) -> Option<f32> {
    let cam = camera.single().ok()?;
    let f = cam.forward();
    let flat = Vec3::new(f.x, 0.0, f.z);
    if flat.length_squared() < 1e-6 {
        return None;
    }
    Some(-std::f32::consts::FRAC_PI_2 - flat.z.atan2(flat.x))
}

/// Rebuild the ring mesh as a **projected decal** — the reference's actual mechanism (clip world
/// geometry to the projection box, texture it top-down), via
/// the shared projector ([`benilla_world::decal::WorldDecal::project`] — the blob shadow rides the same emit
/// chain, `0x6d7330 → 0x6d6fa0 → 0x6d7480`). The ring's box: the *rotated* texture square
/// (half-extent `s` = radius, yawed by the camera fade angle — the clip frame is exactly the
/// texture frame, so UVs stay in `[0,1]`) × the vertical half-range **2s** (the
/// unit ring's box corners are `center±s` horizontal, `center±2s` vertical — `0x608e00`).
/// Vertex alpha is the vertical trapezoid fade: full within ±0.5s of the feet, ramping to 0 at
/// the box's ±2s — so a smear up a wall / down a ledge dims with height the way the director's
/// reference capture shows, instead of ending in a hard clip line. *Interim profile*: the
/// reference's edge-fade alpha grid is byte-located (`0x6147f0`) but its exact ramp is unrecorded
/// (an open question). Runs every shown frame (target moves, camera yaws); the per-surface BVH
/// makes the gather O(log n + k). Returns `false` when nothing was gathered (no ground in the box)
/// — the caller hides the ring, the reference's no-ground gate.
fn project_ring(
    out: &mut Vec<EffectVertex>,
    decals: &WorldDecal<'_, '_>,
    feet: Vec3,
    radius: f32,
    fade_angle: f32,
) -> bool {
    let (sin, cos) = fade_angle.sin_cos();
    let vert = 2.0 * radius;
    let frame = DecalFrame {
        center: feet,
        sin,
        cos,
        min_x: -radius,
        max_x: radius,
        min_z: -radius,
        max_z: radius,
        min_y: -vert,
        max_y: vert,
    };
    decals.project(
        out,
        &frame,
        |p| ((vert - p.y.abs()) / (1.5 * radius)).clamp(0.0, 1.0),
        |x, z| frame.rect_uv(x, z),
    )
}

/// Push the shown ring's cached projection onto the stream, tinted with this frame's resolved
/// colour — one Add draw at the ring rung (the top decal, over the blob shadows), fog off.
pub(super) fn push_ring(
    assets: Option<Res<RingAssets>>,
    state: Option<Res<RingState>>,
    cam: Query<Entity, With<WorldCamera>>,
    mut draw: benilla_world::particles::buffer::WorldEffectDraw,
) {
    let (Some(assets), Some(state)) = (assets, state) else {
        return;
    };
    let Ok(cam) = cam.single() else { return };
    if !state.shown || state.verts.is_empty() {
        return;
    }
    let tint = state.color.to_linear();
    let mut batch = draw
        .batch(cam, assets.texture.id())
        .additive()
        .anchored(state.key.feet)
        .rung(
            benilla_world::sky_order::Rung::RING,
            benilla_world::sky_order::Rung::DECAL_RASTER,
        );
    batch.extend(state.verts.iter().map(|v| EffectVertex {
        pos: v.pos,
        uv: v.uv,
        // The selector's dword as every vertex's diffuse — the reference's own wiring
        // (`0x605960` → vertex colour, alpha = the vertical fade the projector baked).
        color: [tint.red, tint.green, tint.blue, v.color[3]],
    }));
    batch.tris();
}

/// The target's reaction toward our player — the direction the reference colours by (its nameplate/ring
/// resolver `0x7cbaa0` calls `unit->UnitReaction(activePlayer)`, byte-verified; the reverse would read
/// every attackable-but-passive yellow beast as hostile red, since a *player* template is enemy-masked
/// against the whole Monster group), resolved in the client's own order (`0x606530`):
///
/// **First reputation, then the comparator.** If the unit's faction *has a reputation slot*
/// (`FactionHasReputation` `0x605fc0`), the reaction is **our reputation rank** with it
/// (`0x4d63a0`): DBC race/class base + the `SMSG_INITIALIZE_FACTIONS` standing, ranked
/// hated→exalted — *before* any template comparison, which is why every Stormwind NPC is green to a
/// human (base 4000 = friendly) **even in GM mode** (director-verified on the reference: the GM
/// faction template never gets consulted for reputation-faction NPCs). Only a reputation-less
/// faction falls through to the faction-template comparator (byte-exact `0x606640`) over both
/// units' `UNIT_FIELD_FACTIONTEMPLATE`.
///
/// **Before either of those, the duel leg** (byte-exact `UnitReaction 0x6061e0`).
/// The real function runs a player-vs-player ladder *ahead of* the faction work, gated on
/// `UNIT_FIELD_FLAGS` bit 3 (`0x8` `UNIT_FLAG_PVP_ATTACKABLE`, behaviourally "player-controlled")
/// being set on **both** parties. Its first rung is the duel ([`duel_reaction`]): when both
/// players carry a non-zero `PLAYER_DUEL_TEAM` **and** the same `PLAYER_DUEL_ARBITER`, the answer
/// is `1` (hostile) for opposing teams and `4` (friendly) for the same team — `0x606296`'s
/// `setne/dec/and 3/inc`, which maps *equal* → 4 and *unequal* → 1. That is the whole reason a
/// duel opponent turns red, becomes attackable, and takes the combat flash without any of those
/// systems knowing what a duel is. The ladder's later rungs — the party leg (`0x6062b0`) and the
/// both-FFA leg (`0x60632c`) — are NOT implemented here: party colouring already runs through
/// [`ring_variant`]'s own split, and FFA waits for PvP. Both are strictly *below* the duel rung,
/// so their absence cannot change a duel's answer.
///
/// Deferred pieces of the real orchestration: contested-guard flag, forced reactions
/// (`SMSG_SET_FORCED_REACTIONS`), and the summon +1 tail. Returns the **raw reaction rank**
/// (`0..=7` — the scale the ring palette indexes: 0–1 red, 2 orange, 3 yellow, 4–7 green); the
/// comparator's `{1, 3, 4}` sit on the same scale. **3 (neutral)** when anything is missing (no
/// catalog, fields not yet streamed) — the reference resolver's own fall-through is the yellow branch.
pub(crate) fn ring_reaction(
    factions: Option<&Factions>,
    reputations: &Reputations,
    target_store: Option<&ObjectStore>,
    self_store: Option<&ObjectStore>,
) -> u8 {
    if let (Some(target), Some(own)) = (target_store, self_store) {
        if let Some(rank) = duel_reaction(&target.0, &own.0) {
            return rank;
        }
        if ffa_reaction(&target.0, &own.0) {
            return Reaction::Hostile as u8;
        }
    }
    let resolved = (|| {
        let catalog = &factions?.0;
        let self_store = self_store?;
        let target_tpl = catalog.template(target_store?.0.unit_faction_template()?)?;
        // 1. The reputation branch: rank with the unit's faction, when it has a reputation slot.
        if let Some(info) = catalog.reputation_faction(target_tpl.faction) {
            let standing = reputations
                .0
                .get(info.rep_index as usize)
                .map_or(0, |&(_flags, s)| s);
            let race = self_store.0.unit_race().unwrap_or(0);
            let class = self_store.0.unit_class().unwrap_or(0);
            return Some(reputation_rank(info.base_for(race, class) + standing));
        }
        // 2. The faction-template comparator ({1, 3, 4} on the rank scale).
        let self_tpl = catalog.template(self_store.0.unit_faction_template()?)?;
        Some(target_tpl.reaction_toward(self_tpl) as u8)
    })();
    resolved.unwrap_or(Reaction::Neutral as u8)
}

/// `UNIT_FLAG_PVP_ATTACKABLE` — `UNIT_FIELD_FLAGS` bit 3. Behaviourally "player-controlled": the
/// local player and other players carry it, wild creatures do not. `UnitReaction 0x606217`/
/// `0x60622f` requires it on both parties before any player-vs-player rung runs, and `CanAttack
/// 0x606a13` selects its three reaction legs on the same bit.
const UNIT_FLAG_PVP_ATTACKABLE: u32 = 1 << 3;

/// The duel rung of `UnitReaction` (`0x60626b`–`0x6062ad`, byte-exact). `Some(1)` when the two
/// are duelling each other on **opposing** teams, `Some(4)` on the same team (the client's own
/// arithmetic; only pets ever land there), `None` when no duel relates them — the caller then
/// falls through to the faction work.
///
/// The gate is deliberately all three facts and not the arbiter alone: `PLAYER_DUEL_ARBITER` is
/// set on both players the moment the challenge goes out, while `PLAYER_DUEL_TEAM` stays `0`
/// until `Player::UpdateDuelFlag` fires at the end of the countdown. Testing only the arbiter
/// would turn the opponent red during the popup and the 3-second count — before a blow may
/// legally land.
fn duel_reaction(
    target: &benilla_protocol::ObjectFields,
    own: &benilla_protocol::ObjectFields,
) -> Option<u8> {
    match duel_rung(target, own) {
        DuelRung::Engaged { rank, .. } => Some(rank),
        _ => None,
    }
}

/// The **both-FFA** rung of `UnitReaction` (`0x60632c`, the ladder 0633 §5 read in full; decision
/// 0646 §5): two player-controlled units that are BOTH free-for-all flagged are hostile to each
/// other, whatever their factions say. This is the only rung an ordinary PvP flag does *not* have
/// — a same-faction player who flags for PvP stays friendly, which is why there is no `pvp` arm
/// beside this one.
///
/// The flag is `PLAYER_FLAGS` bit 7 (`PLAYER_FLAGS_FFA_PVP`, vmangos `Player.h:322`), the same
/// field and bit the unit snapshot feeds `UnitIsPVPFreeForAll` from.
fn ffa_reaction(
    target: &benilla_protocol::ObjectFields,
    own: &benilla_protocol::ObjectFields,
) -> bool {
    const PLAYER_FLAGS_FFA_PVP: u32 = 0x80;
    let player_controlled =
        |u: &benilla_protocol::ObjectFields| u.unit_flags() & UNIT_FLAG_PVP_ATTACKABLE != 0;
    player_controlled(target)
        && player_controlled(own)
        && target.player_flags() & PLAYER_FLAGS_FFA_PVP != 0
        && own.player_flags() & PLAYER_FLAGS_FFA_PVP != 0
}

/// `UNIT_FIELD_FLAGS` bits `CanAttack` (`0x606980`) tests on its TARGET, any one of which refuses
/// the attack outright — at `0x6069b7`–`0x6069ff`. **The bit NUMBERS are the binary's; the
/// vanilla names are inferred, not confirmed** (a hunt for a labelled consumer of bit 9 found
/// none), so nothing here is written in terms of a name — 1 `0x2`, 7 `0x80`, 16 `0x10000`,
/// 20 `0x100000`, 25 `0x2000000`.
const CANNOT_BE_ATTACKED: u32 = 0x2 | 0x80 | 0x1_0000 | 0x10_0000 | 0x200_0000;
/// The two cross-flag immunity bits `CanAttack` reads on BOTH sides (`0x606a05`–`0x606a8a`): bit 8
/// `0x100` and bit 9 `0x200`, each paired against the other unit's `PLAYER_CONTROLLED` bit 3.
const IMMUNE_TO_PLAYER_CONTROLLED: u32 = 0x100;
const IMMUNE_TO_UNCONTROLLED: u32 = 0x200;
/// `UNIT_FIELD_FLAGS` bit 12 (`0x1000`) — the PvP-flag the both-players arm of `CanAttack` accepts
/// on its target (`0x606b5c`).
const UNIT_FLAG_PVP: u32 = 0x1000;

/// The **local player's** reaction toward a unit — `0x6061e0(this = localPlayer, arg = unit)`.
///
/// **This is NOT [`ring_reaction`] with the arguments swapped, and that is the whole point.** The
/// two directions take genuinely different code inside `0x6061e0`, and the client uses both:
///
/// - **This direction** (`A` = the local player) always reaches leg 3 at `0x606372`, because
///   `0x606170(localPlayer)` resolves to the player themselves. Leg 3 answers a rep-slot faction
///   with the **AT-WAR BIT** — `at_war ? 1 : 4` — and never looks at the standing. Only a
///   faction with no reputation slot falls through to the template comparator.
/// - **[`ring_reaction`]'s direction** (`A` = the unit) never reaches leg 3, falls into `0x606530`,
///   and *there* the standing is read. That is the correct input for the plate's bar COLOUR, the
///   ring, and the overhead name.
///
/// So a not-at-war neutral-standing NPC — a Booty Bay goblin, an Argent Dawn quartermaster, a
/// battleground emissary — is **friendly** to this function and **neutral** to the other one, and
/// the client shows exactly that: a friendly-category plate with a yellow bar. Substituting the
/// standing here is what put those 36 shipped faction templates in the enemy category (1530).
///
/// Deferred, and each one only ever makes a unit *more* friendly than we say: the forced-reaction
/// table (`0x4d6490`, `SMSG_SET_FORCED_REACTIONS` — no wire support yet), the party rung of the
/// player-vs-player block, and the charmed-player case that would stop leg 3 firing at all.
pub(crate) fn reaction_from_player(
    factions: Option<&Factions>,
    reputations: &Reputations,
    target_store: Option<&ObjectStore>,
    self_store: Option<&ObjectStore>,
) -> u8 {
    // The player-vs-player block (`0x606217`) sits ahead of all faction work in BOTH directions,
    // and benilla implements the same two rungs of it here that [`ring_reaction`] does.
    if let (Some(target), Some(own)) = (target_store, self_store) {
        if let Some(rank) = duel_reaction(&target.0, &own.0) {
            return rank;
        }
        if ffa_reaction(&target.0, &own.0) {
            return Reaction::Hostile as u8;
        }
    }
    let resolved = (|| {
        let catalog = &factions?.0;
        let target_tpl = catalog.template(target_store?.0.unit_faction_template()?)?;
        // Leg 3: a faction that owns a reputation slot is answered by the AT-WAR bit alone.
        if let Some(at_war) = at_war_with(catalog, reputations, target_tpl.faction) {
            return Some(if at_war {
                Reaction::Hostile as u8
            } else {
                Reaction::Friendly as u8
            });
        }
        // Leg 4: no reputation slot → the template comparator, PLAYER → UNIT.
        let self_tpl = catalog.template(self_store?.0.unit_faction_template()?)?;
        Some(self_tpl.reaction_toward(target_tpl) as u8)
    })();
    resolved.unwrap_or(Reaction::Neutral as u8)
}

/// **Leg 3's whole content**: is the local player at war with the `Faction.dbc` id a unit's
/// faction template names? `None` when that faction owns no reputation slot — the one case leg 3
/// does not answer, and the reaction falls through to the template comparator.
///
/// `faction_flags::AT_WAR` is the same wire byte the reputation pane's war checkbox writes, which
/// is what makes declaring war on Booty Bay turn its goblins' plates red, move them into the enemy
/// category, and put the sword on the cursor — one bit, three consequences, as the reference has
/// it. Shared with `/reaction`, which prints it: the bit is the cause of every "why is this
/// friendly NPC attackable" question and it is visible nowhere else in the client.
pub(crate) fn at_war_with(
    catalog: &benilla_formats::FactionCatalog,
    reputations: &Reputations,
    faction_id: u32,
) -> Option<bool> {
    let info = catalog.reputation_faction(faction_id)?;
    Some(
        usize::try_from(info.rep_index)
            .ok()
            .and_then(|i| reputations.0.get(i))
            .is_some_and(|&(flags, _)| flags & benilla_formats::faction_flags::AT_WAR != 0),
    )
}

/// `CGUnit::CanInteract(this = the local player → arg = unit)` — `0x6067f0`, complete
/// (disassembly read at `0x6067f0`–`0x606871`, and the register set-up confirmed at every
/// call: `0x4822f2 push esi(unit); mov
/// ecx,edi(player); call 0x606880`, then `0x60691d push esi(unit); mov ecx,edi(player); call
/// 0x6067f0`).
///
/// **This is the world cursor's service gate — not a reaction threshold** (`0x482310`, `test al,al;
/// je 0x4824e6`): TRUE sends the hover into the `UNIT_NPC_FLAGS` service ladder, FALSE into the
/// loot/skin/attack block. Three terms, in the binary's order:
///
/// 1. the unit's `UNIT_FIELD_FLAGS` **bit 25** clear (`0x606835 shr ecx,0x19`),
/// 2. its `UNIT_NPC_FLAGS` **non-zero** (`0x60683d`) — a unit carrying no service bit at all is
///    never interactable, whatever its reaction,
/// 3. **both** reaction directions `>= 3` (neutral), and they are genuinely different functions:
///    `0x606847 push esi; mov ecx,edi; call 0x6061e0` is [`ring_reaction`] (unit → player, the
///    standing), `0x606854 push edi; mov ecx,esi; call 0x6061e0` is [`reaction_from_player`]
///    (player → unit, the **at-war bit**). Each is `cmp eax,3; jl → 0`.
///
/// Term 3's second half is what makes declaring war on a faction drop its vendors out of the
/// service ladder and into the sword — one bit, both consequences, exactly as it moves the plate
/// category (1530).
///
/// **Deferred, and each can only ever make a unit *less* interactable than we say** — so none of
/// them can manufacture a service cursor we would not already show: the `CanInteractNow 0x606880`
/// wrapper's own gates (player `UNIT_FIELD_FLAGS` bit 20 clear @ `0x606893`, both sides'
/// `UNIT_FIELD_CHARMEDBY` zero, player alive, the shapeshift-form clause `0x60e9f0` +
/// `SpellShapeshiftForm` flag `0x8`, and the two unnamed unit predicates `0x613230` / `0x60ecd0`),
/// and `0x6067f0`'s own ghost leg `0x605f70(unit)` — which term 2 already subsumes, since a ghost
/// is a player and a player has no `UNIT_NPC_FLAGS`.
pub(crate) fn can_interact_from_player(
    factions: Option<&Factions>,
    reputations: &Reputations,
    target_store: Option<&ObjectStore>,
    self_store: Option<&ObjectStore>,
) -> bool {
    let Some(target) = target_store else {
        return false; // fields not streamed yet — nothing to take a service from
    };
    const UNIT_FLAG_NOT_SELECTABLE: u32 = 1 << 25;
    if target.0.unit_flags() & UNIT_FLAG_NOT_SELECTABLE != 0 || target.0.unit_npc_flags() == 0 {
        return false;
    }
    const NEUTRAL: u8 = Reaction::Neutral as u8;
    ring_reaction(factions, reputations, target_store, self_store) >= NEUTRAL
        && reaction_from_player(factions, reputations, target_store, self_store) >= NEUTRAL
}

/// `CGUnit::CanAttack(this = the local player → arg = unit)` — `0x606980`, complete. Every leg, in
/// the binary's order.
///
/// This is the predicate the V-plate category actually turns on (see [`plate_is_friendly`]), and
/// it is deliberately **not** a reaction threshold: it disagrees with one in six shipped ways,
/// including an unflagged opposite-faction player on a PvE realm (hostile reaction, cannot be
/// attacked → friendly plate) and a same-faction duel opponent (friendly reaction, can be attacked
/// → enemy plate).
pub(crate) fn can_attack_from_player(
    factions: Option<&Factions>,
    reputations: &Reputations,
    target_store: Option<&ObjectStore>,
    self_store: Option<&ObjectStore>,
    target_is_player: bool,
) -> bool {
    let (Some(target), Some(own)) = (target_store, self_store) else {
        return false; // fields not streamed yet — the reference's own null path refuses
    };
    let (tflags, oflags) = (target.0.unit_flags(), own.0.unit_flags());
    // (a) The ghost gate (`0x606987`): a ghost PLAYER can only be attacked by an attacker holding a
    // creature record whose type-flags carry `0x2` — which a player never does. So for the local
    // player as attacker this leg is unconditional.
    const PLAYER_FLAGS_GHOST: u32 = 0x10;
    if target_is_player && target.0.player_flags() & PLAYER_FLAGS_GHOST != 0 {
        return false;
    }
    // (b) Five target-flag disqualifiers.
    if tflags & CANNOT_BE_ATTACKED != 0 {
        return false;
    }
    // (c) The four cross-flag immunity legs, each pairing one side's immunity bit against the
    // other's `PLAYER_CONTROLLED`.
    let (t_controlled, o_controlled) = (
        tflags & UNIT_FLAG_PVP_ATTACKABLE != 0,
        oflags & UNIT_FLAG_PVP_ATTACKABLE != 0,
    );
    if o_controlled && tflags & IMMUNE_TO_PLAYER_CONTROLLED != 0
        || !o_controlled && tflags & IMMUNE_TO_UNCONTROLLED != 0
        || oflags & IMMUNE_TO_PLAYER_CONTROLLED != 0 && t_controlled
        || oflags & IMMUNE_TO_UNCONTROLLED != 0 && !t_controlled
    {
        return false;
    }
    // (d) The three terminal arms, selected by the two `PLAYER_CONTROLLED` bits.
    let toward_target = || reaction_from_player(factions, reputations, target_store, self_store);
    match (o_controlled, t_controlled) {
        // Neither is player-controlled: hostile in EITHER direction is enough.
        (false, false) => {
            toward_target() <= Reaction::Hostile as u8
                || ring_reaction(factions, reputations, target_store, self_store)
                    <= Reaction::Hostile as u8
        }
        // Both are: the PvP arm. A friendly reaction refuses outright; past that an attack needs a
        // live duel, the target's PvP flag, or mutual FFA. (The charm-owner resolve `0x606170` is
        // an identity for both sides here — benilla has no charm.)
        (true, true) => {
            if toward_target() >= Reaction::Friendly as u8 {
                return false;
            }
            matches!(duel_rung(&target.0, &own.0), DuelRung::Engaged { .. })
                || tflags & UNIT_FLAG_PVP != 0
                || ffa_reaction(&target.0, &own.0)
        }
        // The mixed arm — the local player against any ordinary NPC, i.e. the case the plate gate
        // takes for almost everything on screen: attackable iff worse than friendly.
        _ => toward_target() < Reaction::Friendly as u8,
    }
}

/// The one number `CanCooperate` turns on: a unit's `FactionTemplate` **faction-group mask**
/// (`row + 0xc`) — 3 for every Alliance race row, 5 for every Horde one, and something else
/// entirely for a row that is neither. Exposed for `/reaction`, which prints both sides of the
/// comparison: when a player's plate lands in the wrong bucket this is nearly always the reason,
/// and it is not visible anywhere else in the client.
pub(crate) fn faction_group_mask(
    factions: Option<&Factions>,
    store: Option<&ObjectStore>,
) -> Option<u32> {
    let catalog = &factions?.0;
    Some(
        catalog
            .template(store?.0.unit_faction_template()?)?
            .group_mask,
    )
}

/// `CanCooperate(this = the local player → arg = unit)` — `0x606ba0`: the two `FactionTemplate`
/// rows' **faction-group masks**
/// (`row + 0xc`) being equal, with neither side mind-controlled and the two not being the same
/// unit. It reads no party or raid state.
pub(crate) fn can_cooperate_with_player(
    factions: Option<&Factions>,
    target_store: Option<&ObjectStore>,
    self_store: Option<&ObjectStore>,
) -> bool {
    let (Some(target), Some(own)) = (target_store, self_store) else {
        return false;
    };
    // `606ba6 cmp B,A ; je -> 0` — the predicate's FIRST leg: a unit never cooperates with
    // itself. The reference compares object pointers, so ours compares component identity. The
    // plate gate can't reach it (its query is `Without<SelfPlayer>`), but the predicate is
    // transcribed law and the leg was missing from it.
    if std::ptr::eq(target, own) {
        return false;
    }
    if target.0.unit_charmed_by().is_some_and(|g| g != 0)
        || own.0.unit_charmed_by().is_some_and(|g| g != 0)
    {
        return false;
    }
    let resolved = (|| {
        let catalog = &factions?.0;
        let target_tpl = catalog.template(target.0.unit_faction_template()?)?;
        let self_tpl = catalog.template(own.0.unit_faction_template()?)?;
        Some(target_tpl.group_mask == self_tpl.group_mask)
    })();
    resolved.unwrap_or(false)
}

/// **The V-plate category** — `0x60f6b7`–`0x60f6f1`. `true` = the FRIENDLY bucket (Shift-V's bit
/// `0x8`), `false` =
/// the ENEMY bucket (V's bit `0x1`). There is no reaction rank anywhere in this expression.
///
/// > A unit lands in the FRIENDLY category iff — for a non-player subject —
/// > `CanAttack(localPlayer → subject)` is FALSE; and for a player subject, additionally
/// > `CanCooperate(localPlayer → subject)` is TRUE.
pub(crate) fn plate_is_friendly(
    factions: Option<&Factions>,
    reputations: &Reputations,
    target_store: Option<&ObjectStore>,
    self_store: Option<&ObjectStore>,
    target_is_player: bool,
) -> bool {
    let attackable = can_attack_from_player(
        factions,
        reputations,
        target_store,
        self_store,
        target_is_player,
    );
    if target_is_player {
        can_cooperate_with_player(factions, target_store, self_store) && !attackable
    } else {
        !attackable
    }
}

/// Why [`duel_reaction`]'s rung did or did not fire, with the values it judged on. This is the
/// diagnostic face of the same walk — `/reaction` prints it, so a duel that fails to turn the
/// opponent red names the gate that refused instead of silently reporting "neutral". Keeping one
/// walk with a richer return (rather than a second copy in the probe) is what stops the
/// instrument and the law from drifting apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DuelRung {
    /// One or both sides lack `UNIT_FIELD_FLAGS` bit 3, so no player-vs-player rung runs at all.
    NotPlayerControlled { own: bool, target: bool },
    /// No duel under way yet: `PLAYER_DUEL_TEAM` is still `0` on one or both sides (it is set
    /// only when `Player::UpdateDuelFlag` fires at the end of the countdown).
    NoTeam { own: u32, target: u32 },
    /// Both carry a team, but they are not duelling **each other** — or the arbiter guid has not
    /// streamed to us.
    ArbiterMismatch { own: u64, target: u64 },
    /// The rung fires: `1` (hostile) on opposing teams, `4` (friendly) on the same one.
    Engaged {
        rank: u8,
        own_team: u32,
        target_team: u32,
    },
}

/// The duel rung's walk, reporting the gate that decided (see [`DuelRung`]).
pub(crate) fn duel_rung(
    target: &benilla_protocol::ObjectFields,
    own: &benilla_protocol::ObjectFields,
) -> DuelRung {
    let own_pc = own.unit_flags() & UNIT_FLAG_PVP_ATTACKABLE != 0;
    let target_pc = target.unit_flags() & UNIT_FLAG_PVP_ATTACKABLE != 0;
    if !own_pc || !target_pc {
        return DuelRung::NotPlayerControlled {
            own: own_pc,
            target: target_pc,
        };
    }
    let own_team = own.player_duel_team();
    let target_team = target.player_duel_team();
    if own_team == 0 || target_team == 0 {
        return DuelRung::NoTeam {
            own: own_team,
            target: target_team,
        };
    }
    let arbiter = own.player_duel_arbiter();
    let target_arbiter = target.player_duel_arbiter();
    if arbiter == 0 || arbiter != target_arbiter {
        return DuelRung::ArbiterMismatch {
            own: arbiter,
            target: target_arbiter,
        };
    }
    DuelRung::Engaged {
        rank: if own_team == target_team {
            Reaction::Friendly as u8
        } else {
            Reaction::Hostile as u8
        },
        own_team,
        target_team,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        can_attack_from_player, can_cooperate_with_player, plate_is_friendly, ring_reaction,
        ring_variant, Factions, RingVariant, UNIT_FLAG_PVP_ATTACKABLE,
    };

    /// The selector's player path — the `¬X∧¬Y` split the party arc added (0434 phase 6). A
    /// friendly player reads soft blue solo, pale blue in our party; PvP-flagged reads green
    /// solo, pale green in party; hostile rank still wins for a player; a dead player never grays.
    #[test]
    fn player_ring_party_and_pvp_split() {
        let v = |pvp, in_party| ring_variant(5, true, false, pvp, in_party);
        assert_eq!(
            v(false, false),
            RingVariant::Player,
            "friendly solo = soft blue"
        );
        assert_eq!(
            v(false, true),
            RingVariant::Party,
            "friendly party = pale blue"
        );
        assert_eq!(v(true, false), RingVariant::Friendly, "pvp solo = green");
        assert_eq!(
            v(true, true),
            RingVariant::PartyPvp,
            "pvp party = pale green"
        );
        // A hostile-rank player is red regardless of party/pvp (the X/Y approximation).
        assert_eq!(
            ring_variant(1, true, false, true, true),
            RingVariant::Hostile
        );
        // Players skip the health check — a dead ally still reads its party colour, never gray.
        assert_eq!(ring_variant(5, true, true, false, true), RingVariant::Party);
    }

    /// The NPC path is untouched by the party inputs: dead grays, else the reaction palette,
    /// and pvp/in_party never apply to a non-player.
    #[test]
    fn npc_ring_ignores_party_inputs() {
        assert_eq!(ring_variant(0, false, true, true, true), RingVariant::Dead);
        assert_eq!(
            ring_variant(2, false, false, true, true),
            RingVariant::Unfriendly
        );
        assert_eq!(
            ring_variant(3, false, false, false, false),
            RingVariant::Neutral
        );
        assert_eq!(
            ring_variant(5, false, false, true, true),
            RingVariant::Friendly
        );
    }

    /// **The plate category, end to end on the REAL build-5875 DBC — the two subjects the director
    /// observed on the reference client** (1530), plus the leg that actually discriminates the
    /// mechanism.
    ///
    /// This is the regression pin the old `rank >= 4` model could never have passed. It asserts the
    /// *divergence itself*: the emissary's reaction rank stays 3 (neutral — which is right, and is
    /// what paints its bar yellow) while its plate CATEGORY is friendly. A future refactor that
    /// "simplifies" the category back into a rank threshold fails here, on real data, naming the
    /// creature.
    #[test]
    fn the_plate_category_reproduces_the_reference_on_the_real_dbc() {
        use crate::net::{ObjectStore, Reputations};
        use benilla_protocol::ObjectFields;

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let factions = Factions(benilla_formats::load_faction_catalog(&mut chain).expect("dbc"));
        // Field indices: 35 = UNIT_FIELD_FACTIONTEMPLATE, 46 = UNIT_FIELD_FLAGS.
        let unit = |tpl: u32| ObjectStore(ObjectFields::from_pairs(&[(35, tpl)]));
        // Us: faction template 1 (PLAYER, Human), carrying `PLAYER_CONTROLLED`.
        let me = ObjectStore(ObjectFields::from_pairs(&[
            (35, 1),
            (46, UNIT_FLAG_PVP_ATTACKABLE),
        ]));
        let quiet = Reputations::default(); // nothing at war — the out-of-box state
        let category = |unit: &ObjectStore, reps: &Reputations| {
            plate_is_friendly(Some(&factions), reps, Some(unit), Some(&me), false)
        };
        let rank = |unit: &ObjectStore, reps: &Reputations| {
            ring_reaction(Some(&factions), reps, Some(unit), Some(&me))
        };

        // A Chicken (FT 31 → faction 28 "Prey", NO reputation slot) falls to the template
        // comparator, which matches nothing → 3 → attackable → the ENEMY bucket, plain V. The
        // director confirmed this on the reference: critters do plate, and nothing excludes them.
        let chicken = unit(31);
        assert!(!category(&chicken, &quiet), "a critter is enemy-category");
        assert_eq!(rank(&chicken, &quiet), 3, "and its bar is neutral yellow");

        // A League of Arathor Emissary (FT 1577 → faction 509, reputation slot 53) is answered by
        // the AT-WAR bit — not at war → 4 → not attackable → the FRIENDLY bucket, Shift-V only.
        // The director confirmed exactly this on the reference.
        let emissary = unit(1577);
        assert!(category(&emissary, &quiet), "friendly-category at neutral");
        assert_eq!(
            rank(&emissary, &quiet),
            3,
            "…while the BAR stays yellow: the two halves run the reaction in opposite \
             directions, and this disagreement is the reference's own"
        );

        // **The discriminating population** (36 shipped templates where the at-war leg
        // and the mask comparison disagree). Booty Bay's mask answer is 3 — so if this read as
        // enemy-category we would be back on the comparator, and the at-war leg would be fiction.
        let goblin = unit(120);
        assert!(category(&goblin, &quiet), "Booty Bay: friendly, not at war");
        // …and declaring war flips it, which is the same bit the reputation pane's checkbox writes.
        let mut slots = vec![(0u8, 0i32); 64];
        slots[1] = (benilla_formats::faction_flags::AT_WAR, 0); // Booty Bay = slot 1
        let at_war = Reputations(slots);
        assert!(!category(&goblin, &at_war), "at war → enemy category");
        assert!(
            category(&emissary, &at_war),
            "…and only that faction's own bit moves"
        );

        // A Stormwind guard (FT 12 → faction 72, slot 19) is friendly by BOTH routes — the case
        // that agreed all along, kept so the fix cannot be read as having moved it.
        assert!(category(&unit(12), &quiet));
        assert_eq!(rank(&unit(12), &quiet), 4);
    }

    /// **The reported bug, on the real build-5875 DBC, naming the faction** (1674): hovering a
    /// **Cenarion Circle** NPC with no gossip drew the ATTACK sword at neutral standing, with the
    /// faction not set to war.
    ///
    /// Cenarion Circle is faction 609, reputation slot 36 — one of the 36 shipped templates
    /// decided entirely by the at-war bit, and one 1530 named while correcting the
    /// plate category alone. The cursor and `relations::can_attack` kept the reaction *threshold*,
    /// which reads the OTHER direction (the standing, 0 → neutral → "≤ 3 is attackable"), so the
    /// sword came back for every one of those factions.
    ///
    /// The pin asserts the divergence the threshold cannot express: **the rank stays 3 — the ring
    /// and the bar are untouched — while both branch predicates say Point.** A "simplification"
    /// back to a threshold has to break this, on real data, naming the creature.
    #[test]
    fn a_not_at_war_cenarion_circle_npc_takes_no_cursor_at_neutral() {
        use crate::net::{ObjectStore, Reputations};
        use benilla_protocol::ObjectFields;

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let factions = Factions(benilla_formats::load_faction_catalog(&mut chain).expect("dbc"));
        // 35 = UNIT_FIELD_FACTIONTEMPLATE, 46 = UNIT_FIELD_FLAGS, 147 = UNIT_NPC_FLAGS.
        let npc = |tpl: u32, npc_flags: u32| {
            ObjectStore(ObjectFields::from_pairs(&[(35, tpl), (147, npc_flags)]))
        };
        let me = ObjectStore(ObjectFields::from_pairs(&[
            (35, 1), // Human
            (46, UNIT_FLAG_PVP_ATTACKABLE),
        ]));
        let quiet = Reputations::default(); // nothing at war — the out-of-box state
        let mut slots = vec![(0u8, 0i32); 64];
        slots[36] = (benilla_formats::faction_flags::AT_WAR, 0); // Cenarion Circle = slot 36
        let at_war = Reputations(slots);

        let attackable = |u: &ObjectStore, r: &Reputations| {
            can_attack_from_player(Some(&factions), r, Some(u), Some(&me), false)
        };
        let interactable = |u: &ObjectStore, r: &Reputations| {
            crate::target::ring::can_interact_from_player(Some(&factions), r, Some(u), Some(&me))
        };

        // FactionTemplate 996 → faction 609 "Cenarion Circle" (also 635/994/1254/1608). The
        // reported subject: no service bit at all, so the ladder was never in play — the unit fell
        // straight to the attack leg.
        let silent = npc(996, 0);
        assert_eq!(
            ring_reaction(Some(&factions), &quiet, Some(&silent), Some(&me)),
            3,
            "neutral standing — the ring and the bar are RIGHT and must not move"
        );
        assert!(
            !attackable(&silent, &quiet),
            "not at war → CanAttack says no: the sword was the bug"
        );
        assert!(
            !interactable(&silent, &quiet),
            "and with no service bit it is not interactable either → the cursor CLEARS to Point"
        );

        // A Cenarion Circle NPC that DOES gossip keeps its speech bubble — the control that must
        // not move, and the half of the report that was already right.
        let gossip = npc(996, crate::target::cursor_mode::npc_flags::GOSSIP);
        assert!(interactable(&gossip, &quiet), "gossip NPC still talks");
        assert!(!attackable(&gossip, &quiet));

        // Declaring war moves BOTH, off one bit: leg 3 answers hostile, so the gossiper drops out
        // of the service ladder and the silent one takes the sword. This is the behaviour the
        // director named as the discriminator — "even if you don't have the faction set to at war".
        assert!(
            attackable(&silent, &at_war),
            "at war → the sword is correct"
        );
        assert!(attackable(&gossip, &at_war));
        assert!(
            !interactable(&gossip, &at_war),
            "at war → no more gossip cursor; the reference leaves the ladder entirely"
        );
        // …and only that faction's own slot moves.
        assert_eq!(
            ring_reaction(Some(&factions), &at_war, Some(&silent), Some(&me)),
            3,
            "the STANDING is still neutral — at-war does not touch the bar"
        );

        // **The control that must not change**: an ordinary neutral creature whose faction owns no
        // reputation slot (Chicken, FT 31 → faction 28 "Prey") is answered by the template
        // comparator, reads neutral, and is still attackable. Neutral mobs keep the sword — the fix
        // is about the at-war bit, not about neutrality.
        let chicken = npc(31, 0);
        assert!(
            attackable(&chicken, &quiet),
            "a neutral critter still takes the sword — the comparator, not at-war"
        );
        assert!(!interactable(&chicken, &quiet));
    }

    /// **The PLAYER subject** — the arm 1530 landed and left unpinned, on the real DBC.
    ///
    /// A player is friendly-category only if `CanCooperate` ALSO says yes, and that predicate is
    /// pure `FactionTemplate` faction-group-mask equality (`0x606ba0`). So the arm turns entirely
    /// on one number — `group_mask` 3 for every Alliance race template, 5 for every Horde one — and
    /// the interesting population is the templates that carry NEITHER.
    ///
    /// **`35` is that template, and it is what a vmangos test realm hands you**: `.gm on` calls
    /// `Player::SetGameMaster(true)` → `SetFactionTemplateId(35)` (vmangos `Player.cpp:2656`), whose
    /// row is `faction 31, group_mask 0`. Zero equals no player's mask, so a GM-mode character —
    /// on either side of the pair — falls out of the friendly bucket and takes a plate under plain
    /// V. Nothing else on screen moves, because `CanCooperate` is only ever consulted for a player
    /// subject. That asymmetric signature is what a "players are plating and mobs look fine"
    /// report means, and this test is where to read it. (`GM.LoginState = 2` keeps the mode across
    /// logins, so it outlives the session that set it.)
    #[test]
    fn the_player_subject_category_on_the_real_dbc() {
        use crate::net::{ObjectStore, Reputations};
        use benilla_protocol::ObjectFields;

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let factions = Factions(benilla_formats::load_faction_catalog(&mut chain).expect("dbc"));
        let reps = Reputations::default();
        // Every player carries `PLAYER_CONTROLLED`; the plate gate passes `is_player = true`.
        let player = |tpl: u32| {
            ObjectStore(ObjectFields::from_pairs(&[
                (35, tpl),
                (46, UNIT_FLAG_PVP_ATTACKABLE),
            ]))
        };
        let category = |subject: &ObjectStore, me: &ObjectStore| {
            plate_is_friendly(Some(&factions), &reps, Some(subject), Some(me), true)
        };

        // Us: a Human (FT 1, group_mask 3).
        let me = player(1);
        // Same faction group — a Gnome is FT 115, a different row with the SAME mask. Friendly
        // category: no plate under plain V, which is the shipped behaviour this pins.
        assert!(
            category(&player(115), &me),
            "an Alliance player is friendly"
        );
        assert!(category(&player(1), &me), "…same race, same answer");
        // The other faction group (Orc FT 2, mask 5) is enemy-category — and that is FAITHFUL:
        // the reference plates the opposing faction under plain V, flagged or not.
        assert!(!category(&player(2), &me), "a Horde player is enemy");
        // A GM-mode player (FT 35, mask 0) matches NOBODY's mask — enemy category from a normal
        // character, while `CanAttack` still refuses (the plate is drawn over someone unattackable).
        assert!(!category(&player(35), &me), "a GM-mode player is enemy");
        assert!(
            !can_attack_from_player(Some(&factions), &reps, Some(&player(35)), Some(&me), true),
            "…and not because we can attack them",
        );

        // The mirror: a GM-mode OBSERVER loses the friendly bucket for every player at once…
        let gm = player(35);
        for tpl in [1u32, 3, 4, 115, 2, 5, 6, 116] {
            assert!(
                !category(&player(tpl), &gm),
                "FT {tpl} is enemy-category to a GM-mode observer",
            );
        }
        // …while nothing else on screen moves: `CanCooperate` is never consulted for an NPC, so a
        // Stormwind guard stays friendly and a Monster-faction mob stays enemy. THAT asymmetry is
        // the report's signature.
        let npc = |tpl: u32| ObjectStore(ObjectFields::from_pairs(&[(35, tpl)]));
        let npc_category = |subject: &ObjectStore, me: &ObjectStore| {
            plate_is_friendly(Some(&factions), &reps, Some(subject), Some(me), false)
        };
        assert!(npc_category(&npc(12), &gm), "the guard keeps his bucket");
        assert!(!npc_category(&npc(14), &gm), "and the mob keeps his");

        // The predicate's first leg (`606ba6`): nobody cooperates with themselves.
        assert!(!can_cooperate_with_player(
            Some(&factions),
            Some(&me),
            Some(&me)
        ));
    }

    /// `CanAttack`'s flag disqualifiers put a unit in the FRIENDLY bucket **at any reaction** — the
    /// class of case a rank threshold gets wrong in the other direction (`0x6069b7`–`0x6069ff`).
    /// Asserted on
    /// the mask path (a Monster-faction template, hostile by reaction) so the reaction is
    /// unambiguously hostile and only the flag can be doing the work.
    #[test]
    fn an_unattackable_flag_beats_a_hostile_reaction() {
        use crate::net::{ObjectStore, Reputations};
        use benilla_protocol::ObjectFields;

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let factions = Factions(benilla_formats::load_faction_catalog(&mut chain).expect("dbc"));
        let me = ObjectStore(ObjectFields::from_pairs(&[
            (35, 1),
            (46, UNIT_FLAG_PVP_ATTACKABLE),
        ]));
        let reps = Reputations::default();
        let mob = |flags: u32| ObjectStore(ObjectFields::from_pairs(&[(35, 14), (46, flags)]));

        // FT 14 "Monster" — hostile to the player by the mask comparison.
        assert_eq!(
            ring_reaction(Some(&factions), &reps, Some(&mob(0)), Some(&me)),
            1
        );
        assert!(
            !plate_is_friendly(Some(&factions), &reps, Some(&mob(0)), Some(&me), false),
            "a plain hostile mob is enemy-category"
        );
        // Each of the five refusal bits alone flips the category, hostile reaction and all.
        for bit in [0x2u32, 0x80, 0x1_0000, 0x10_0000] {
            assert!(
                plate_is_friendly(Some(&factions), &reps, Some(&mob(bit)), Some(&me), false),
                "flag {bit:#x} refuses the attack → friendly category"
            );
        }
        // Bit 8 (0x100) refuses only because WE are player-controlled — the cross-flag leg.
        assert!(plate_is_friendly(
            Some(&factions),
            &reps,
            Some(&mob(0x100)),
            Some(&me),
            false
        ));
    }

    /// **A torn-down object stops being an object at the teardown, not when its model is done
    /// fading.** The reference's two teardown routes (`SMSG_DESTROY_OBJECT`, the OUT_OF_RANGE
    /// block) both reach `0x464920` → OnDeactivate `0x5fbb60` → `0x493910`, which clears a
    /// matching selection and sends `CMSG_SET_SELECTION 0` on the spot; only the detached model
    /// survives into the `SWModelFadeout` scheduler. Driven through the real
    /// handler table on the built client, then the real
    /// ring and the real nearest-enemy scan, one pass each — no fade time elapses.
    mod teardown {
        use benilla_protocol::field::{
            FIELD_UNIT_FLAGS, FIELD_UNIT_HEALTH, FIELD_UNIT_LEVEL, FIELD_UNIT_MAXHEALTH,
        };
        use benilla_protocol::messages::ObjectType;
        use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};
        use bevy::ecs::system::RunSystemOnce;
        use bevy::prelude::*;
        use crossbeam_channel::Receiver;

        use crate::net::{ClientCommand, Guid, GuidIndex, NetCommands, ObjectStore, SelfPlayer};
        use crate::target::{attack_order_target, Selection, TargetScan};
        use benilla_world::model_fade::DespawnFade;

        const ME: u64 = 0x0000_0000_0000_0007;
        const MOB: u64 = 0xF130_0000_1234_0001;

        fn create(guid: u64) -> SessionEvent {
            SessionEvent::ObjectCreate {
                guid,
                kind: EntityKind::Unit,
                display_id: None,
                position: [3.0, 0.0, 0.0],
                orientation: 0.0,
                scale: 1.0,
                speeds: None,
                mover: None,
                transport_progress: None,
                transport: None,
                spline: None,
                fields: ObjectFields::from_pairs(&[
                    (FIELD_UNIT_HEALTH, 100),
                    (FIELD_UNIT_MAXHEALTH, 100),
                    (FIELD_UNIT_LEVEL, 9),
                ])
                .into_created(ObjectType::Unit),
            }
        }

        /// The built client with our own avatar and one live mob in the index, the mob
        /// selected, and the write channel captured.
        fn client_with_selected_mob() -> (App, Entity, Receiver<ClientCommand>) {
            let mut app = crate::game_plugins::schedule_tests::headless_client();
            let (tx, rx) = crossbeam_channel::unbounded();
            app.insert_resource(NetCommands(tx));
            let world = app.world_mut();
            // Inserted by a Startup system on a real boot; no schedule runs here.
            world.init_resource::<super::super::RingState>();
            let me = world
                .spawn((
                    SelfPlayer,
                    Guid(ME),
                    Transform::default(),
                    ObjectStore(
                        // Player-controlled (`UNIT_FLAG_PVP_ATTACKABLE`), so a neutral mob is
                        // attackable with no faction catalog loaded — the scan's control half.
                        ObjectFields::from_pairs(&[
                            (FIELD_UNIT_HEALTH, 100),
                            (FIELD_UNIT_MAXHEALTH, 100),
                            (FIELD_UNIT_FLAGS, 0x8),
                        ])
                        .into_created(ObjectType::Player),
                    ),
                ))
                .id();
            world.resource_mut::<GuidIndex>().0.insert(ME, me);
            crate::net::handlers::dispatch(world, vec![create(MOB)]);
            let mob = world.resource::<GuidIndex>().0[&MOB];
            *world.resource_mut::<Selection>() = Selection {
                target: Some(mob),
                guid: Some(MOB),
            };
            // The control: a live selected unit keeps its selection through a ring pass.
            world
                .run_system_once(super::super::update_ring)
                .expect("the ring runs on the built client");
            assert_eq!(world.resource::<Selection>().guid, Some(MOB));
            // (The create's own name query is on the channel; only a selection send matters.)
            assert!(
                !rx.try_iter()
                    .any(|c| matches!(c, ClientCommand::SetSelection { .. })),
                "a live target is not deselected"
            );
            (app, mob, rx)
        }

        fn selection_cleared_at(teardown: SessionEvent) {
            let (mut app, mob, rx) = client_with_selected_mob();
            let world = app.world_mut();
            crate::net::handlers::dispatch(world, vec![teardown]);
            // The model is still there, fading — the scheduler's half of the teardown.
            assert!(
                world.get_entity(mob).is_ok(),
                "the model outlives the object"
            );
            assert!(world.get::<DespawnFade>(mob).is_some(), "…and fades");
            world
                .run_system_once(super::super::update_ring)
                .expect("the ring runs on the built client");
            let sel = world.resource::<Selection>();
            assert_eq!(
                (sel.target, sel.guid),
                (None, None),
                "the selection ends with the object, not 2 s later with its model"
            );
            let sent: Vec<_> = rx.try_iter().collect();
            assert!(
                sent.iter()
                    .any(|c| matches!(c, ClientCommand::SetSelection { guid: 0 })),
                "the server is told at the teardown: {sent:?}"
            );
        }

        #[test]
        fn a_destroyed_target_clears_the_selection_at_the_destroy() {
            selection_cleared_at(SessionEvent::ObjectDestroyed(MOB));
        }

        #[test]
        fn a_streamed_out_target_clears_the_selection_at_the_stream_out() {
            selection_cleared_at(SessionEvent::ObjectsRemoved(vec![MOB]));
        }

        /// The nearest-enemy acquire (the TAB core) cannot pick a fading model: it is not an
        /// object any more. The control half proves the scan does find the mob while it lives.
        #[test]
        fn the_nearest_enemy_scan_never_picks_a_torn_down_object() {
            let (mut app, _mob, _rx) = client_with_selected_mob();
            let world = app.world_mut();
            let acquire = |world: &mut World| {
                *world.resource_mut::<Selection>() = Selection::default();
                world
                    .run_system_once(
                        |scan: TargetScan,
                         mut sel: ResMut<Selection>,
                         mut seam: crate::creature_anim::AttackSeam,
                         mut errors: ResMut<crate::ui_action::UiErrorKeys>| {
                            attack_order_target(&scan, &mut sel, &mut seam, &mut errors)
                        },
                    )
                    .expect("the scan runs on the built client")
            };
            assert_eq!(
                acquire(world),
                Some(MOB),
                "control: the live mob is acquired"
            );
            crate::net::handlers::dispatch(world, vec![SessionEvent::ObjectsRemoved(vec![MOB])]);
            assert_eq!(acquire(world), None, "the fading model is not a candidate");
        }

        /// The corpse → respawn shape: the server destroys the old object and creates the same
        /// guid again in one tick. The fresh create is a new live object, and the old model
        /// fading beside it no longer answers to the guid.
        #[test]
        fn a_same_tick_recreate_is_a_fresh_object_and_the_old_model_is_nobody() {
            let (mut app, old, _rx) = client_with_selected_mob();
            let world = app.world_mut();
            crate::net::handlers::dispatch(
                world,
                vec![SessionEvent::ObjectDestroyed(MOB), create(MOB)],
            );
            let fresh = world.resource::<GuidIndex>().0[&MOB];
            assert_ne!(fresh, old, "a fresh entity for the fresh object");
            assert!(world.get::<DespawnFade>(fresh).is_none());
            let answering: Vec<Entity> = world
                .query::<(Entity, &Guid)>()
                .iter(world)
                .filter(|(_, g)| g.0 == MOB)
                .map(|(e, _)| e)
                .collect();
            assert_eq!(answering, [fresh], "one object answers to the guid");
        }
    }
}
