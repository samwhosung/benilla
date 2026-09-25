//! The ground selection ring under the current target: the reference's `UnitSelectTexture.blp`
//! projected onto the terrain and WMO faces in its box, never doodads or GameObjects (`0x6723b0`,
//! flags `0x200122`), additive, steady and at full brightness from the instant of selection, its
//! bright arc toward the camera. The vertical fade on wall and ledge pieces is matched to
//! captures: the reference's edge-fade ramp (`0x6147f0`) is untraced.

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

/// The ring record: `verts` is rebuilt when the [`RingKey`] changes, `color` every frame.
#[derive(Resource, Default)]
pub(super) struct RingState {
    verts: Vec<EffectVertex>,
    key: RingKey,
    /// This frame's tint: the selector's colour or the combat flash's.
    color: Color,
    shown: bool,
}

/// The projection's inputs; while they hold still, the cached projection is reused.
#[derive(Default, PartialEq, Clone, Copy)]
struct RingKey {
    feet: Vec3,
    radius: f32,
    fade_angle: f32,
    surfaces: usize,
}

/// The ring texture; the draw waits until it loads.
#[derive(Resource)]
pub(super) struct RingAssets {
    texture: Handle<Image>,
}

/// The reference's selection-circle texture, loaded at `0x6146d0`: a white ring, tinted per vertex.
const RING_TEXTURE: &str = "mpq://textures/unitselecttexture.blp";
/// Ring radius for a unit with no model: the reference's degenerate-box answer, 1.2 (`0x60aee0`).
const RING_FALLBACK_RADIUS: f32 = benilla_formats::DEGENERATE_RING_FOOTPRINT;
/// The selector's own palette (`0x605960`), not the nameplate's, written as each decal vertex's
/// diffuse: NPCs by reaction rank 0-1 red, 2 orange `0xFFFF8000`, 3 yellow, 4-7 green, dead gray
/// `0xFF7F7F7F`; players soft blue `0xFF6060FF` or, PvP-flagged, green; party members (the table
/// `0xbc6f48`) pale blue `0xFFAAAAFF` or pale green `0xFFAAFFAA`.
// `linear_rgb` passes the authored bytes raw: the framebuffer is gamma-encoded.
const RING_HOSTILE: Color = Color::linear_rgb(1.0, 0.0, 0.0);
const RING_UNFRIENDLY: Color = Color::linear_rgb(1.0, 0.502, 0.0);
const RING_NEUTRAL: Color = Color::linear_rgb(1.0, 1.0, 0.0);
const RING_FRIENDLY: Color = Color::linear_rgb(0.0, 1.0, 0.0);
const RING_PLAYER: Color = Color::linear_rgb(0.376, 0.376, 1.0);
const RING_DEAD: Color = Color::linear_rgb(0.498, 0.498, 0.498);
const RING_PARTY: Color = Color::linear_rgb(0.667, 0.667, 1.0); // 0xFFAAAAFF
const RING_PARTY_PVP: Color = Color::linear_rgb(0.667, 1.0, 0.667); // 0xFFAAFFAA

/// The FactionTemplate.dbc catalog; absent if it failed to load, and the faction legs then read
/// neutral.
#[derive(Resource)]
pub(crate) struct Factions(FactionCatalog);

impl Factions {
    pub(crate) fn catalog(&self) -> &FactionCatalog {
        &self.0
    }

    /// Wrap a catalog, for tests; a running client builds the resource once, from the DBC.
    #[cfg(test)]
    pub(crate) fn from_catalog(catalog: FactionCatalog) -> Self {
        Self(catalog)
    }
}

/// The colour class `GetSelectionCircleColor` resolves, shared by the ground ring and the overhead
/// name, which both read the selector (`vtable+0x2c`).
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

/// The selector's branch (`0x605960`) on the raw reaction rank `0..=7`. A player never grays: rank
/// 0-1 reads red, standing in for the reference's attackability matrix, else green when PvP-flagged
/// and blue when not, paler for a party member; self is not in the party table. An NPC reads gray
/// when dead, else the rank palette. The reference takes the player branch on `UNIT_FIELD_FLAGS`
/// bit 3 (`0x605baa`), which a player's pet carries too; callers pass the object type.
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

/// Load the ring texture and seed the ring record.
pub(super) fn setup_ring(mut commands: Commands, asset_server: Res<AssetServer>) {
    let texture = asset_server.load::<Image>(RING_TEXTURE);
    commands.insert_resource(RingAssets { texture });
    commands.init_resource::<RingState>();
}

/// Load FactionTemplate.dbc once the MPQ chain is open; on failure the resource stays absent.
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

/// Place, size and colour the ring under the current target, or hide it: radius the model's ring
/// footprint ([`SelectionRadius`]) times its scale, colour resolved every frame as faction can
/// change live.
#[allow(clippy::type_complexity)]
pub(super) fn update_ring(
    mut selection: ResMut<Selection>,
    factions: Option<Res<Factions>>,
    reputations: Res<Reputations>,
    flash: Res<CombatFlash>,
    mut state: ResMut<RingState>,
    camera: Query<&GlobalTransform, With<WorldCamera>>,
    decals: WorldDecal,
    // Kept so a straight-down camera holds the last fade angle.
    mut fade_angle: Local<f32>,
    mut logged_guid: Local<Option<u64>>,
    // Last frame's (guid, dead), tracked every frame: a respawned creature reuses its guid, so
    // state armed only on a selection change would miss its second death.
    mut last_vitals: Local<Option<(u64, bool)>>,
    mut seam: crate::creature_anim::AttackSeam,
    // Net entities are roots, so `Transform` is this frame's world position. `.0` filters on
    // `Guid`, which a torn-down unit sheds at the teardown while its model fades on. `.1` gives a
    // mounted target the mount model's footprint at its rendered scale, as the reference's ring
    // cache (`+0xcf0`) recomputes from the mount (`0x60ce70` → `0x60aee0`).
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
        // The party roster: the selector's party table `0xbc6f48`.
        Res<crate::ui_party::GroupState>,
    ),
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    // Our auto-attack; both clear paths end it, as the reference's death and teardown edges do.
    engaged: Query<(), (With<Engaged>, With<SelfPlayer>)>,
) {
    let state = &mut *state;
    let hide = match selection.target {
        None => {
            *last_vitals = None;
            *logged_guid = None;
            true
        }
        Some(target) => match targets.0.get(target) {
            Ok((unit, sel_radius, store, net, mount_child)) => {
                // The footprint has no floor, as in the reference: a zero-bounds model already
                // measures the degenerate-box 1.2.
                let (local, mount_scale) = match mount_child.and_then(|mc| targets.1.get(mc.0).ok())
                {
                    Some((mnet, msel)) => (msel.map_or(RING_FALLBACK_RADIUS, |r| r.0), mnet.scale),
                    None => (sel_radius.map_or(RING_FALLBACK_RADIUS, |r| r.0), 1.0),
                };
                let radius = local * (unit.scale.x * mount_scale).max(0.01);
                *fade_angle = ring_fade_angle(&camera).unwrap_or(*fade_angle);
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
                // The unit's own PvP flag (`0x1000`); the reference reads its charmer's or
                // summoner's when it has one.
                let pvp = store.is_some_and(|s| s.0.unit_flags() & 0x1000 != 0);
                let in_party = selection
                    .guid
                    .is_some_and(|g| targets.2.members.iter().any(|m| m.guid == g));
                let is_dead = store.is_some_and(|s| s.0.unit_is_dead());
                // The alive-to-dead edge clears the target, as the reference's health handler does
                // (`0x6046f0` fires `0x605860`, which sends `CMSG_SET_SELECTION 0`); a corpse
                // selected afterwards stays selected, gray.
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
                // The combat flash, the selector's first branch (`0x605960`), outranks the rest.
                state.color = if flash.unit == Some(target) {
                    flash.color
                } else {
                    ring_variant(rank, is_player, is_dead, pvp, in_party).color()
                };
                // No ground in the box hides the ring: the reference skips the draw (`0x6d74b5`).
                !projected
            }
            // No longer an object: clear and tell the server, as the reference's deactivate does on
            // both removal paths (`0x5fbb60` → `0x493910`). This also drops a selected corpse at
            // respawn, which the server destroys before the fresh create.
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

/// The fade's camera-relative angle θ, turning the texture's faded side away from the camera as
/// the reference's camera-fed projector does. The texture is bright toward ring-local +Z and clear
/// toward −Z, and [`project_ring`] rotates the UVs by −θ, so `θ = −π/2 − atan2(f.z, f.x)` for the
/// camera's ground-projected forward `f`. `None` looking straight down.
fn ring_fade_angle(camera: &Query<&GlobalTransform, With<WorldCamera>>) -> Option<f32> {
    let cam = camera.single().ok()?;
    let f = cam.forward();
    let flat = Vec3::new(f.x, 0.0, f.z);
    if flat.length_squared() < 1e-6 {
        return None;
    }
    Some(-std::f32::consts::FRAC_PI_2 - flat.z.atan2(flat.x))
}

/// Project the ring through the reference's decal projector (`0x6d7330` → `0x6d6fa0` →
/// `0x6d7480`): the box is the texture square, yawed by the fade angle, reaching two radii up and
/// down (`0x608e00`). Vertex alpha is full within half a radius of the feet and falls to 0 at the
/// box's top and bottom. False when nothing is gathered.
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

/// Draw the cached projection tinted, one additive draw on the ring rung above the blob shadows.
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
        // The selector's colour as each vertex's diffuse, as in the reference; alpha is the fade.
        color: [tint.red, tint.green, tint.blue, v.color[3]],
    }));
    batch.tris();
}

/// The target's reaction toward our player as a raw rank `0..=7`, the direction every NPC colour
/// uses (`0x605960` and `0x7cbaa0` ask `UnitReaction 0x6061e0` with the unit as `this`). When
/// both carry `UNIT_FIELD_FLAGS` bit 3, a duel (`0x606296`) or mutual FFA decides first. Then
/// `0x606530`: a faction with a reputation slot (`0x605fc0`) answers with our reputation rank
/// (`0x4d63a0`), even in GM mode; any other goes to the template comparator (`0x606640`). Neutral
/// when anything is missing. Not applied: the party rung (`0x6062b0`), the contested guard,
/// forced reactions and the summon tail.
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
        // A reputation faction: our rank with it.
        if let Some(info) = catalog.reputation_faction(target_tpl.faction) {
            let standing = reputations
                .0
                .get(info.rep_index as usize)
                .map_or(0, |&(_flags, s)| s);
            let race = self_store.0.unit_race().unwrap_or(0);
            let class = self_store.0.unit_class().unwrap_or(0);
            return Some(reputation_rank(info.base_for(race, class) + standing));
        }
        // The template comparator: 1, 3 or 4 on the rank scale.
        let self_tpl = catalog.template(self_store.0.unit_faction_template()?)?;
        Some(target_tpl.reaction_toward(self_tpl) as u8)
    })();
    resolved.unwrap_or(Reaction::Neutral as u8)
}

/// `UNIT_FIELD_FLAGS` bit 3, player-controlled in behaviour: players and their pets carry it, wild
/// creatures do not. `UnitReaction` needs it on both sides before any player-vs-player rung
/// (`0x606217`, `0x60622f`), and `CanAttack` picks its reaction arm by it (`0x606a13`).
const UNIT_FLAG_PVP_ATTACKABLE: u32 = 1 << 3;

/// The duel rung of `UnitReaction` (`0x60626b`–`0x6062ad`): hostile (1) between opposing teams,
/// friendly (4) on one team, `None` when no duel relates the two. It needs team and arbiter both:
/// `PLAYER_DUEL_ARBITER` is set at the challenge, `PLAYER_DUEL_TEAM` only when the countdown ends
/// (`Player::UpdateDuelFlag`), so the arbiter alone would turn the opponent red during the count.
fn duel_reaction(
    target: &benilla_protocol::ObjectFields,
    own: &benilla_protocol::ObjectFields,
) -> Option<u8> {
    match duel_rung(target, own) {
        DuelRung::Engaged { rank, .. } => Some(rank),
        _ => None,
    }
}

/// The both-FFA rung of `UnitReaction` (`0x60632c`): two player-controlled units both flagged
/// free-for-all (`PLAYER_FLAGS` bit 7, vmangos `Player.h:322`) are hostile whatever their factions.
/// A plain PvP flag has no such rung: a same-faction player flagged for PvP stays friendly.
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

/// Target `UNIT_FIELD_FLAGS` bits, any one of which makes `CanAttack` refuse
/// (`0x6069b7`–`0x6069ff`); their vanilla names are unconfirmed, so none is used.
const CANNOT_BE_ATTACKED: u32 = 0x2 | 0x80 | 0x1_0000 | 0x10_0000 | 0x200_0000;
/// The cross-flag immunity bits `CanAttack` reads on both sides (`0x606a05`–`0x606a8a`), each
/// against the other unit's bit 3.
const IMMUNE_TO_PLAYER_CONTROLLED: u32 = 0x100;
const IMMUNE_TO_UNCONTROLLED: u32 = 0x200;
/// The PvP flag, which `CanAttack`'s player-vs-player arm accepts on its target (`0x606b5c`).
const UNIT_FLAG_PVP: u32 = 0x1000;

/// The local player's reaction toward a unit, `0x6061e0` with the player as `this`. This direction
/// reaches leg 3 (`0x606372`), which answers a reputation faction by the at-war bit alone, never
/// the standing, so a not-at-war neutral NPC is friendly here and neutral to [`ring_reaction`]: a
/// friendly-category plate with a yellow bar. Not applied, each only ever making a unit
/// friendlier: forced reactions (`0x4d6490`), the party rung and the charmed-player case.
pub(crate) fn reaction_from_player(
    factions: Option<&Factions>,
    reputations: &Reputations,
    target_store: Option<&ObjectStore>,
    self_store: Option<&ObjectStore>,
) -> u8 {
    // The player-vs-player block (`0x606217`) runs first in both directions.
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
        // Leg 3: a reputation faction answers by the at-war bit alone.
        if let Some(at_war) = at_war_with(catalog, reputations, target_tpl.faction) {
            return Some(if at_war {
                Reaction::Hostile as u8
            } else {
                Reaction::Friendly as u8
            });
        }
        // Leg 4: the template comparator, player toward unit.
        let self_tpl = catalog.template(self_store?.0.unit_faction_template()?)?;
        Some(self_tpl.reaction_toward(target_tpl) as u8)
    })();
    resolved.unwrap_or(Reaction::Neutral as u8)
}

/// Leg 3: whether we are at war with the `Faction.dbc` faction a unit's template names; `None` when
/// it has no reputation slot. The bit is the one the reputation pane's war checkbox writes, so
/// declaring war moves the plate, the cursor and attackability at once. `/reaction` prints it.
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

/// `CanInteract` from the local player (`0x6067f0`), the world cursor's service gate at
/// `0x482310`: bit 25 of `UNIT_FIELD_FLAGS` clear (`0x606835`), some `UNIT_NPC_FLAGS`
/// (`0x60683d`), and both reactions at least neutral, [`ring_reaction`] (`0x606847`) and
/// [`reaction_from_player`] (`0x606854`). Not applied, each only ever making a unit less
/// interactable: the `CanInteractNow 0x606880` wrapper's gates (`0x606893`, charm, alive,
/// shapeshift `0x60e9f0`, `0x613230`, `0x60ecd0`) and the ghost leg `0x605f70`.
pub(crate) fn can_interact_from_player(
    factions: Option<&Factions>,
    reputations: &Reputations,
    target_store: Option<&ObjectStore>,
    self_store: Option<&ObjectStore>,
) -> bool {
    let Some(target) = target_store else {
        return false; // fields not streamed yet
    };
    const UNIT_FLAG_NOT_SELECTABLE: u32 = 1 << 25;
    if target.0.unit_flags() & UNIT_FLAG_NOT_SELECTABLE != 0 || target.0.unit_npc_flags() == 0 {
        return false;
    }
    const NEUTRAL: u8 = Reaction::Neutral as u8;
    ring_reaction(factions, reputations, target_store, self_store) >= NEUTRAL
        && reaction_from_player(factions, reputations, target_store, self_store) >= NEUTRAL
}

/// `CanAttack` from the local player (`0x606980`), every leg in the reference's order; the V-plate
/// category turns on it. Not a reaction threshold: an unflagged opposite-faction player on a PvE
/// realm is hostile yet not attackable, a same-faction duel opponent friendly yet attackable.
pub(crate) fn can_attack_from_player(
    factions: Option<&Factions>,
    reputations: &Reputations,
    target_store: Option<&ObjectStore>,
    self_store: Option<&ObjectStore>,
    target_is_player: bool,
) -> bool {
    let (Some(target), Some(own)) = (target_store, self_store) else {
        return false; // fields not streamed yet; the reference's null path refuses too
    };
    let (tflags, oflags) = (target.0.unit_flags(), own.0.unit_flags());
    // (a) The ghost gate (`0x606987`): only a creature whose type flags carry `0x2` may attack a
    // ghost player, so the local player never can.
    const PLAYER_FLAGS_GHOST: u32 = 0x10;
    if target_is_player && target.0.player_flags() & PLAYER_FLAGS_GHOST != 0 {
        return false;
    }
    // (b) Five target-flag disqualifiers.
    if tflags & CANNOT_BE_ATTACKED != 0 {
        return false;
    }
    // (c) The four cross-flag immunity legs.
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
    // (d) The three terminal arms, selected by the two bit-3 flags.
    let toward_target = || reaction_from_player(factions, reputations, target_store, self_store);
    match (o_controlled, t_controlled) {
        // Neither player-controlled: hostile in either direction suffices.
        (false, false) => {
            toward_target() <= Reaction::Hostile as u8
                || ring_reaction(factions, reputations, target_store, self_store)
                    <= Reaction::Hostile as u8
        }
        // Both: a friendly reaction refuses; else a live duel, the target's PvP flag or mutual
        // FFA. The reference's charm-owner resolve (`0x606170`) is not applied.
        (true, true) => {
            if toward_target() >= Reaction::Friendly as u8 {
                return false;
            }
            matches!(duel_rung(&target.0, &own.0), DuelRung::Engaged { .. })
                || tflags & UNIT_FLAG_PVP != 0
                || ffa_reaction(&target.0, &own.0)
        }
        // Mixed, the player against an ordinary NPC: attackable when worse than friendly.
        _ => toward_target() < Reaction::Friendly as u8,
    }
}

/// A unit's `FactionTemplate` faction-group mask (`row + 0xc`), the one number `CanCooperate`
/// compares: 3 on every Alliance race row, 5 on every Horde one. `/reaction` prints it.
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

/// `CanCooperate` from the local player (`0x606ba0`): equal faction-group masks, neither side
/// charmed, and not the same unit; it reads no party or raid state.
pub(crate) fn can_cooperate_with_player(
    factions: Option<&Factions>,
    target_store: Option<&ObjectStore>,
    self_store: Option<&ObjectStore>,
) -> bool {
    let (Some(target), Some(own)) = (target_store, self_store) else {
        return false;
    };
    // The first leg: a unit never cooperates with itself (`0x606ba6` compares the objects).
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

/// The V-plate category (`0x60f6b7`–`0x60f6f1`): `true` is the friendly bucket (Shift-V, bit
/// `0x8`), `false` the enemy one (V, bit `0x1`). A unit is friendly when `CanAttack` refuses it,
/// and a player only when `CanCooperate` also passes; no reaction rank takes part.
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

/// Why [`duel_reaction`]'s rung did or did not fire, with the values it judged; `/reaction` prints
/// it from this same walk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DuelRung {
    /// A side lacks `UNIT_FIELD_FLAGS` bit 3, so no player-vs-player rung runs.
    NotPlayerControlled { own: bool, target: bool },
    /// `PLAYER_DUEL_TEAM` is 0 on a side: no duel, or its countdown has not ended.
    NoTeam { own: u32, target: u32 },
    /// Both have a team but are not duelling each other, or the arbiter has not streamed.
    ArbiterMismatch { own: u64, target: u64 },
    /// The rung fires: 1 (hostile) on opposing teams, 4 (friendly) on one team.
    Engaged {
        rank: u8,
        own_team: u32,
        target_team: u32,
    },
}

/// The duel rung's walk, reporting the gate that decided.
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
        assert_eq!(
            ring_variant(1, true, false, true, true),
            RingVariant::Hostile
        );
        // A dead player never grays.
        assert_eq!(ring_variant(5, true, true, false, true), RingVariant::Party);
    }

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

    /// The emissary's rank stays neutral, which paints its bar yellow, while its plate category is
    /// friendly: the two run the reaction in opposite directions.
    #[test]
    fn the_plate_category_reproduces_the_reference_on_the_real_dbc() {
        use crate::net::{ObjectStore, Reputations};
        use benilla_protocol::ObjectFields;

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let factions = Factions(benilla_formats::load_faction_catalog(&mut chain).expect("dbc"));
        // Field indices: 35 = UNIT_FIELD_FACTIONTEMPLATE, 46 = UNIT_FIELD_FLAGS.
        let unit = |tpl: u32| ObjectStore(ObjectFields::from_pairs(&[(35, tpl)]));
        // Us: a player-controlled Human, faction template 1.
        let me = ObjectStore(ObjectFields::from_pairs(&[
            (35, 1),
            (46, UNIT_FLAG_PVP_ATTACKABLE),
        ]));
        let quiet = Reputations::default(); // nothing at war
        let category = |unit: &ObjectStore, reps: &Reputations| {
            plate_is_friendly(Some(&factions), reps, Some(unit), Some(&me), false)
        };
        let rank = |unit: &ObjectStore, reps: &Reputations| {
            ring_reaction(Some(&factions), reps, Some(unit), Some(&me))
        };

        // A Chicken, template 31 with no reputation slot: neutral, attackable, the enemy bucket.
        let chicken = unit(31);
        assert!(!category(&chicken, &quiet), "a critter is enemy-category");
        assert_eq!(rank(&chicken, &quiet), 3, "and its bar is neutral yellow");

        // A League of Arathor Emissary, template 1577, reputation slot 53, not at war.
        let emissary = unit(1577);
        assert!(category(&emissary, &quiet), "friendly-category at neutral");
        assert_eq!(
            rank(&emissary, &quiet),
            3,
            "…while the BAR stays yellow: the two halves run the reaction in opposite \
             directions, and this disagreement is the reference's own"
        );

        // Booty Bay, template 120: the comparator says 3, so only the at-war leg makes it friendly.
        let goblin = unit(120);
        assert!(category(&goblin, &quiet), "Booty Bay: friendly, not at war");
        let mut slots = vec![(0u8, 0i32); 64];
        slots[1] = (benilla_formats::faction_flags::AT_WAR, 0); // Booty Bay = slot 1
        let at_war = Reputations(slots);
        assert!(!category(&goblin, &at_war), "at war → enemy category");
        assert!(
            category(&emissary, &at_war),
            "…and only that faction's own bit moves"
        );

        // A Stormwind guard, template 12, faction 72, slot 19: friendly by both routes.
        assert!(category(&unit(12), &quiet));
        assert_eq!(rank(&unit(12), &quiet), 4);
    }

    /// Cenarion Circle, faction 609 with reputation slot 36: not at war, its NPCs are neither
    /// attackable nor, without a service bit, interactable, while the rank stays neutral.
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
        let quiet = Reputations::default(); // nothing at war
        let mut slots = vec![(0u8, 0i32); 64];
        slots[36] = (benilla_formats::faction_flags::AT_WAR, 0); // Cenarion Circle = slot 36
        let at_war = Reputations(slots);

        let attackable = |u: &ObjectStore, r: &Reputations| {
            can_attack_from_player(Some(&factions), r, Some(u), Some(&me), false)
        };
        let interactable = |u: &ObjectStore, r: &Reputations| {
            crate::target::ring::can_interact_from_player(Some(&factions), r, Some(u), Some(&me))
        };

        // Template 996, faction 609, with no service bit.
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

        // One that gossips keeps its speech bubble.
        let gossip = npc(996, crate::target::cursor_mode::npc_flags::GOSSIP);
        assert!(interactable(&gossip, &quiet), "gossip NPC still talks");
        assert!(!attackable(&gossip, &quiet));

        // At war: the gossiper leaves the ladder, the silent one takes the sword.
        assert!(
            attackable(&silent, &at_war),
            "at war → the sword is correct"
        );
        assert!(attackable(&gossip, &at_war));
        assert!(
            !interactable(&gossip, &at_war),
            "at war → no more gossip cursor; the reference leaves the ladder entirely"
        );
        assert_eq!(
            ring_reaction(Some(&factions), &at_war, Some(&silent), Some(&me)),
            3,
            "the STANDING is still neutral — at-war does not touch the bar"
        );

        // A Chicken, template 31 with no reputation slot, still takes the sword.
        let chicken = npc(31, 0);
        assert!(
            attackable(&chicken, &quiet),
            "a neutral critter still takes the sword — the comparator, not at-war"
        );
        assert!(!interactable(&chicken, &quiet));
    }

    /// A player is friendly only if `CanCooperate`, faction-group-mask equality (`0x606ba0`), also
    /// passes. Template 35, which vmangos's `.gm on` sets (`Player.cpp:2661`), is faction 31 with
    /// mask 0, so a GM-mode character leaves every player's friendly bucket while NPCs keep theirs.
    #[test]
    fn the_player_subject_category_on_the_real_dbc() {
        use crate::net::{ObjectStore, Reputations};
        use benilla_protocol::ObjectFields;

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let factions = Factions(benilla_formats::load_faction_catalog(&mut chain).expect("dbc"));
        let reps = Reputations::default();
        let player = |tpl: u32| {
            ObjectStore(ObjectFields::from_pairs(&[
                (35, tpl),
                (46, UNIT_FLAG_PVP_ATTACKABLE),
            ]))
        };
        let category = |subject: &ObjectStore, me: &ObjectStore| {
            plate_is_friendly(Some(&factions), &reps, Some(subject), Some(me), true)
        };

        // Us: a Human, template 1, mask 3.
        let me = player(1);
        // A Gnome, template 115: another row with the same mask.
        assert!(
            category(&player(115), &me),
            "an Alliance player is friendly"
        );
        assert!(category(&player(1), &me), "…same race, same answer");
        // An Orc, template 2, mask 5: the enemy bucket, flagged or not.
        assert!(!category(&player(2), &me), "a Horde player is enemy");
        // A GM-mode player, template 35, mask 0, matches no one: enemy, though not attackable.
        assert!(!category(&player(35), &me), "a GM-mode player is enemy");
        assert!(
            !can_attack_from_player(Some(&factions), &reps, Some(&player(35)), Some(&me), true),
            "…and not because we can attack them",
        );

        // A GM-mode observer loses the friendly bucket for every player…
        let gm = player(35);
        for tpl in [1u32, 3, 4, 115, 2, 5, 6, 116] {
            assert!(
                !category(&player(tpl), &gm),
                "FT {tpl} is enemy-category to a GM-mode observer",
            );
        }
        // …while NPCs keep theirs: `CanCooperate` is never asked for an NPC.
        let npc = |tpl: u32| ObjectStore(ObjectFields::from_pairs(&[(35, tpl)]));
        let npc_category = |subject: &ObjectStore, me: &ObjectStore| {
            plate_is_friendly(Some(&factions), &reps, Some(subject), Some(me), false)
        };
        assert!(npc_category(&npc(12), &gm), "the guard keeps his bucket");
        assert!(!npc_category(&npc(14), &gm), "and the mob keeps his");

        // Nobody cooperates with themselves (`0x606ba6`).
        assert!(!can_cooperate_with_player(
            Some(&factions),
            Some(&me),
            Some(&me)
        ));
    }

    /// `CanAttack`'s flag refusals (`0x6069b7`–`0x6069ff`) beat a hostile reaction.
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

        // Template 14, Monster: hostile by the comparator.
        assert_eq!(
            ring_reaction(Some(&factions), &reps, Some(&mob(0)), Some(&me)),
            1
        );
        assert!(
            !plate_is_friendly(Some(&factions), &reps, Some(&mob(0)), Some(&me), false),
            "a plain hostile mob is enemy-category"
        );
        // Each of these refusal bits alone flips the category.
        for bit in [0x2u32, 0x80, 0x1_0000, 0x10_0000] {
            assert!(
                plate_is_friendly(Some(&factions), &reps, Some(&mob(bit)), Some(&me), false),
                "flag {bit:#x} refuses the attack → friendly category"
            );
        }
        // Bit 8 (`0x100`) refuses because we are player-controlled: the cross-flag leg.
        assert!(plate_is_friendly(
            Some(&factions),
            &reps,
            Some(&mob(0x100)),
            Some(&me),
            false
        ));
    }

    /// A torn-down object stops being an object at the teardown, not when its model has faded:
    /// both of the reference's routes (`SMSG_DESTROY_OBJECT`, the out-of-range block) reach
    /// `0x464920` → `0x5fbb60` → `0x493910`, which clears a matching selection and sends
    /// `CMSG_SET_SELECTION 0` at once, while only the detached model fades on.
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

        /// The built client with our avatar and one selected live mob, the write channel captured.
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
                        // Player-controlled: a neutral mob is attackable with no faction catalog.
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
            // Control: a live target keeps its selection through a ring pass.
            world
                .run_system_once(super::super::update_ring)
                .expect("the ring runs on the built client");
            assert_eq!(world.resource::<Selection>().guid, Some(MOB));
            // The create's name query is on the channel too; only a selection send matters.
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

        /// The nearest-enemy scan never picks a fading model.
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

        /// A respawn destroys and re-creates the guid in one tick: the create is a new object, and
        /// the fading model does not answer to the guid.
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
