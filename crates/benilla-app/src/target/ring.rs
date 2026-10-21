//! The ground selection ring under the current target: the reference's `UnitSelectTexture.blp`
//! projected onto the terrain and WMO faces in its box, never doodads or GameObjects (`0x6723b0`,
//! flags `0x200122`), additive, steady and at full brightness from the instant of selection, its
//! bright arc toward the camera. The vertical fade on wall and ledge pieces is matched to
//! captures: the reference's edge-fade ramp (`0x6147f0`) is untraced.

use benilla_formats::{load_faction_catalog, FactionCatalog, Reaction};
use benilla_protocol::EntityKind;
use bevy::prelude::*;

use crate::net::{Guid, NetEntity, ObjectStore, SelfPlayer};
use benilla_assets::{LockRecover, WorldAssets};
use benilla_world::decal::{DecalFrame, WorldDecal};
use benilla_world::particles::buffer::EffectVertex;

use super::click::deselect;
use super::relations::{RelUnit, RelationWorld, Relations};
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
/// `0xFF7F7F7F`; players red, yellow, soft blue `0xFF6060FF` or green, and party members (the
/// table `0xbc6f48`) pale blue `0xFFAAAAFF` or pale green `0xFFAAFFAA` ([`ring_variant`]).
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
/// neutral. Shared, so the Lua API's copy of the relation inputs costs no table.
#[derive(Resource, Clone)]
pub(crate) struct Factions(std::sync::Arc<FactionCatalog>);

impl Factions {
    pub(crate) fn catalog(&self) -> &FactionCatalog {
        &self.0
    }

    /// Wrap a catalog, for tests; a running client builds the resource once, from the DBC.
    #[cfg(test)]
    pub(crate) fn from_catalog(catalog: FactionCatalog) -> Self {
        Self(std::sync::Arc::new(catalog))
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

/// The selector's player-path inputs (`0x605bb6`–`0x605cf6`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct PlayerPath {
    /// X, `CanAttack(unit → player)` (`0x605bb9`).
    pub(crate) attacks_us: bool,
    /// Y, `CanAttack(player → unit)` (`0x605bc8`, `0x605c0b`).
    pub(crate) attackable: bool,
    /// `IsPvP 0x605ff0` (`0x605c32`): the `UNIT_FLAG_PVP` of the unit's charmer, else its
    /// creator, when it has one held, and no for such a unit flagged `0x100`; else its own.
    pub(crate) pvp: bool,
    /// `0x4e7f70`: you or a member of the party table `0xbc6f48`; the selector tests itself first
    /// (`0x605c46`), so here it is a member.
    pub(crate) in_party: bool,
}

/// What the selector (`0x605960`) branches on below the combat flash.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SelectorInput {
    /// The player path, which never grays.
    Player(PlayerPath),
    /// Any other unit: its reaction toward the player ([`Relations::reaction`], `0..=7`), and
    /// whether it is dead.
    Npc { rank: u8, dead: bool },
}

/// The selector's colour class. The player path: X∧Y red, X∧¬Y soft blue, ¬X∧Y yellow
/// (`0x605bb6`–`0x605c2c`), else green when PvP-flagged and blue when not, paler for a party
/// member. Any other unit: gray when dead, else the rank palette. The reference takes the player
/// path on `UNIT_FIELD_FLAGS` bit 3 (`0x605baa`), which a player's pet carries too; callers pass
/// the object type.
pub(crate) fn ring_variant(input: SelectorInput) -> RingVariant {
    match input {
        SelectorInput::Player(p) => match (p.attacks_us, p.attackable, p.pvp, p.in_party) {
            (true, true, ..) => RingVariant::Hostile,
            (true, false, ..) => RingVariant::Player,
            (false, true, ..) => RingVariant::Neutral,
            (false, false, true, true) => RingVariant::PartyPvp,
            (false, false, true, false) => RingVariant::Friendly,
            (false, false, false, true) => RingVariant::Party,
            (false, false, false, false) => RingVariant::Player,
        },
        SelectorInput::Npc { dead: true, .. } => RingVariant::Dead,
        SelectorInput::Npc { rank, .. } => match rank {
            0..=1 => RingVariant::Hostile,
            2 => RingVariant::Unfriendly,
            3 => RingVariant::Neutral,
            _ => RingVariant::Friendly,
        },
    }
}

/// `GetSelectionCircleColor 0x605960` below the combat flash, over the unit and the active player:
/// the ground ring and the overhead name both paint with it. A unit whose descriptor has not
/// streamed reads as neither attacking nor attackable, neutral and alive.
pub(crate) fn selection_variant(
    rel: &Relations,
    unit: Option<&RelUnit>,
    me: Option<&RelUnit>,
    is_player: bool,
    dead: bool,
    in_party: bool,
) -> RingVariant {
    let pair = unit.zip(me);
    let attack = |attacker_is_unit: bool| {
        pair.is_some_and(|(unit, me)| {
            if attacker_is_unit {
                rel.can_attack(unit, me)
            } else {
                rel.can_attack(me, unit)
            }
        })
    };
    ring_variant(if is_player {
        SelectorInput::Player(PlayerPath {
            attacks_us: attack(true),
            attackable: attack(false),
            // `IsPvP` of the unit (`0x605c32`).
            pvp: unit.is_some_and(|u| rel.is_pvp(u)),
            in_party,
        })
    } else {
        SelectorInput::Npc {
            rank: pair.map_or(Reaction::Neutral as u8, |(unit, me)| rel.reaction(unit, me)),
            dead,
        }
    })
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
            commands.insert_resource(Factions(std::sync::Arc::new(catalog)));
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
    world: RelationWorld,
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
    // Our auto-attack, which the death clear ends, as the reference's death edge does.
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
                let rel = world.get();
                let me = world.me();
                let subject = store.zip(selection.guid).map(|(s, g)| world.unit_of(g, s));
                let rank = subject
                    .zip(me)
                    .map_or(Reaction::Neutral as u8, |(u, me)| rel.reaction(&u, &me));
                let is_player = net.is_some_and(|n| n.kind == EntityKind::Player);
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
                    // `SetSelection(0,0)` at `0x605901`, so the dead target becomes the last one.
                    deselect(&mut selection, &mut seam, !engaged.is_empty());
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
                        me.and_then(|m| m.template),
                        if is_player { " [player path]" } else { "" },
                        if is_dead { " [dead→gray]" } else { "" },
                    );
                }
                // The combat flash, the selector's first branch (`0x605960`), outranks the rest.
                state.color = if flash.unit == Some(target) {
                    flash.color
                } else {
                    selection_variant(
                        &rel,
                        subject.as_ref(),
                        me.as_ref(),
                        is_player,
                        is_dead,
                        in_party,
                    )
                    .color()
                };
                // No ground in the box hides the ring: the reference skips the draw (`0x6d74b5`).
                !projected
            }
            // No longer an object: nothing to draw. The selection's own teardown is the net stage's
            // (`track::follow_selection_object`).
            Err(_) => {
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

/// The V-plate category (`0x60f6b7`–`0x60f6f1`): `true` is the friendly bucket (Shift-V, bit
/// `0x8`), `false` the enemy one (V, bit `0x1`). A unit is friendly when `CanAttack` from the
/// active player refuses it, and a player only when `CanCooperate` also passes; no reaction rank
/// takes part.
pub(crate) fn plate_is_friendly(
    rel: &Relations,
    unit: &RelUnit,
    me: &RelUnit,
    is_player: bool,
) -> bool {
    let attackable = rel.can_attack(me, unit);
    if is_player {
        rel.can_cooperate(me, unit) && !attackable
    } else {
        !attackable
    }
}

#[cfg(test)]
mod tests {
    use super::{plate_is_friendly, ring_variant, selection_variant, PlayerPath, RingVariant};
    use super::{Factions, SelectorInput};
    use crate::net::Reputations;
    use crate::target::relations::fixture::*;
    use crate::target::relations::RelUnit;

    const ME: u64 = 0x10;
    const OTHER: u64 = 0x11;
    const NPC: u64 = 0x20;

    fn shipped_factions() -> Option<Factions> {
        let data = benilla_formats::wow_data_or_skip!(None);
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        Some(Factions::from_catalog(
            benilla_formats::load_faction_catalog(&mut chain).expect("faction tables"),
        ))
    }

    /// The selector's player path (`0x605bb6`-`0x605cf6`): `CanAttack` both ways, X the unit
    /// attacking us and Y us attacking it. X∧Y red, X∧¬Y blue, ¬X∧Y yellow, else the PvP split.
    /// No catalog here, so every faction leg reads neutral and only the flags decide.
    #[test]
    fn the_player_path_is_the_mutual_attackability_matrix() {
        let variant = |unit: RelUnit, me: RelUnit, in_party| {
            let mut scene = Scene::new(me, None);
            let unit = scene.hold(unit);
            selection_variant(&scene.rel(), Some(&unit), Some(&me), true, false, in_party)
        };
        // Yourself in the free-for-all ring keeps your PvP colour: neither direction attacks.
        let me = flagged(player(ME, HUMAN), FFA);
        assert_eq!(
            variant(me, me, false),
            RingVariant::Player,
            "unflagged self"
        );
        let me = with_flags(me, PVP);
        assert_eq!(
            variant(me, me, false),
            RingVariant::Friendly,
            "flagged self"
        );
        // Another player in the ring: both free-for-all, both attack.
        let ffa = |guid| flagged(player(guid, HUMAN), FFA);
        assert_eq!(variant(ffa(OTHER), ffa(ME), false), RingVariant::Hostile);
        // Outside it, each side attacks the other only when that other is flagged.
        let open = |guid, flags| with_flags(player(guid, HUMAN), flags);
        let (them, us) = (|f| open(OTHER, f), |f| open(ME, f));
        assert_eq!(
            variant(them(PVP), us(PVP), false),
            RingVariant::Hostile,
            "X∧Y"
        );
        assert_eq!(
            variant(them(0), us(PVP), false),
            RingVariant::Player,
            "X∧¬Y"
        );
        assert_eq!(
            variant(them(PVP), us(0), false),
            RingVariant::Neutral,
            "¬X∧Y"
        );
        assert_eq!(variant(them(0), us(0), false), RingVariant::Player, "¬X∧¬Y");
        // The party split rides the last arm only.
        assert_eq!(variant(them(0), us(0), true), RingVariant::Party);
        assert_eq!(variant(them(PVP), us(PVP), true), RingVariant::Hostile);
    }

    /// The PvP split asks `IsPvP` of the unit (`0x605c32`), which answers with a charmer's flag:
    /// a flagged player we mind-control reads as our unflagged self, blue, not green, and green
    /// once we are flagged. Neither side attacks the other: our controller is his.
    #[test]
    fn a_mind_controlled_player_takes_the_pvp_split_off_his_charmer() {
        let variant = |me: RelUnit| {
            let mut scene = Scene::new(me, None);
            let thrall = scene.hold(RelUnit {
                charmed_by: ME,
                ..with_flags(player(OTHER, HUMAN), PVP)
            });
            selection_variant(&scene.rel(), Some(&thrall), Some(&me), true, false, false)
        };
        assert_eq!(variant(player(ME, HUMAN)), RingVariant::Player);
        assert_eq!(
            variant(with_flags(player(ME, HUMAN), PVP)),
            RingVariant::Friendly
        );
    }

    /// On the shipped templates, a new human toward an Orc (2) and a Gnome (115): the Horde
    /// factions are at war from the first reputation list, so a flagged Orc attacks and is
    /// attacked, and an ally never is, flagged or not.
    #[test]
    fn enemy_and_ally_players_read_by_attackability_on_the_real_dbc() {
        let Some(factions) = shipped_factions() else {
            return;
        };
        let reputations = new_human_reputations(factions.catalog());
        let mut scene = Scene::new(human_warrior(ME), Some(factions));
        scene.reputations = reputations;
        let mut variant = |unit: RelUnit, me: RelUnit| {
            scene.hold(me);
            let unit = scene.hold(unit);
            selection_variant(&scene.rel(), Some(&unit), Some(&me), true, false, false)
        };
        let (me, me_flagged) = (human_warrior(ME), with_flags(human_warrior(ME), PVP));
        let orc = |flags| with_flags(player(OTHER, ORC), flags);
        assert_eq!(variant(orc(PVP), me_flagged), RingVariant::Hostile);
        assert_eq!(variant(orc(0), me_flagged), RingVariant::Player);
        assert_eq!(variant(orc(PVP), me), RingVariant::Neutral);
        assert_eq!(variant(orc(0), me), RingVariant::Player);
        let gnome = |flags| with_flags(player(OTHER, 115), flags);
        assert_eq!(variant(gnome(PVP), me_flagged), RingVariant::Friendly);
        assert_eq!(variant(gnome(0), me), RingVariant::Player);
        // Yourself in the ring, on the shipped templates too.
        let me = flagged(me_flagged, FFA);
        assert_eq!(variant(me, me), RingVariant::Friendly);
    }

    /// The four player-path arms (`0x605bb6`-`0x605cf6`), and the party split on the last.
    #[test]
    fn player_ring_matrix_party_and_pvp_split() {
        let v = |attacks_us, attackable, pvp, in_party| {
            ring_variant(SelectorInput::Player(PlayerPath {
                attacks_us,
                attackable,
                pvp,
                in_party,
            }))
        };
        assert_eq!(v(true, true, true, true), RingVariant::Hostile, "X∧Y red");
        assert_eq!(v(true, false, true, true), RingVariant::Player, "X∧¬Y blue");
        assert_eq!(
            v(false, true, true, true),
            RingVariant::Neutral,
            "¬X∧Y yellow"
        );
        assert_eq!(
            v(false, false, false, false),
            RingVariant::Player,
            "solo blue"
        );
        assert_eq!(
            v(false, false, false, true),
            RingVariant::Party,
            "party pale blue"
        );
        assert_eq!(v(true, false, false, false), RingVariant::Player);
        assert_eq!(
            v(false, false, true, false),
            RingVariant::Friendly,
            "pvp green"
        );
        assert_eq!(
            v(false, false, true, true),
            RingVariant::PartyPvp,
            "pale green"
        );
    }

    #[test]
    fn npc_ring_reads_rank_and_death() {
        let v = |rank, dead| ring_variant(SelectorInput::Npc { rank, dead });
        assert_eq!(v(0, true), RingVariant::Dead);
        assert_eq!(v(5, true), RingVariant::Dead);
        assert_eq!(v(1, false), RingVariant::Hostile);
        assert_eq!(v(2, false), RingVariant::Unfriendly);
        assert_eq!(v(3, false), RingVariant::Neutral);
        assert_eq!(v(5, false), RingVariant::Friendly);
    }

    /// The emissary's rank stays neutral, which paints its bar yellow, while its plate category is
    /// friendly: the two run the reaction in opposite directions.
    #[test]
    fn the_plate_category_reproduces_the_reference_on_the_real_dbc() {
        let Some(factions) = shipped_factions() else {
            return;
        };
        let me = player(ME, HUMAN);
        let mut scene = Scene::new(me, Some(factions));
        // (category, the unit's rank toward us) under a reputation list.
        let mut judge = |unit: RelUnit, reps: Reputations| {
            scene.reputations = reps;
            let rel = scene.rel();
            (
                plate_is_friendly(&rel, &unit, &me, false),
                rel.reaction(&unit, &me),
            )
        };
        let quiet = Reputations::default;
        let forced = |table| {
            let mut reps = Reputations::default();
            reps.set_forced(table);
            reps
        };
        let at_war = || {
            let mut slots = vec![(0u8, 0i32); 64];
            slots[1] = (benilla_formats::faction_flags::AT_WAR, 0); // Booty Bay = slot 1
            Reputations::from_slots(slots)
        };

        // A Chicken, template 31 with no reputation slot: neutral, attackable, the enemy bucket.
        let chicken = creature(NPC, 31);
        assert_eq!(judge(chicken, quiet()), (false, 3));
        // `SMSG_SET_FORCED_REACTIONS` naming the chicken's faction (28) makes it friendly, ahead
        // of the templates; a table naming another faction does not.
        assert_eq!(judge(chicken, forced(vec![(28, 4)])), (true, 4));
        assert_eq!(judge(chicken, forced(vec![(29, 4)])), (false, 3));

        // A League of Arathor Emissary, template 1577, reputation slot 53, not at war: friendly
        // category while the bar stays yellow.
        let emissary = creature(NPC + 1, 1577);
        assert_eq!(judge(emissary, quiet()), (true, 3));

        // Booty Bay, template 120: the comparator says 3, so only the at-war leg makes it friendly.
        let goblin = creature(NPC + 2, 120);
        assert!(judge(goblin, quiet()).0, "Booty Bay: friendly, not at war");
        assert!(!judge(goblin, at_war()).0, "at war: the enemy category");
        assert!(
            judge(emissary, at_war()).0,
            "only that faction's own bit moves"
        );

        // A Stormwind guard, template 12, faction 72, slot 19: friendly by both routes.
        assert_eq!(judge(creature(NPC + 3, 12), quiet()), (true, 4));
    }

    /// A player is friendly only if `CanCooperate`, faction-group-mask equality (`0x606ba0`), also
    /// passes. Template 35, which vmangos's `.gm on` sets (`Player.cpp:2661`), is faction 31 with
    /// mask 0, so a GM-mode character leaves every player's friendly bucket while NPCs keep theirs.
    #[test]
    fn the_player_subject_category_on_the_real_dbc() {
        let Some(factions) = shipped_factions() else {
            return;
        };
        // Us: a Human, template 1, mask 3.
        let me = player(ME, HUMAN);
        let mut scene = Scene::new(me, Some(factions));
        let mut category = |subject: RelUnit, me: RelUnit, is_player| {
            scene.hold(me);
            let subject = scene.hold(subject);
            let rel = scene.rel();
            let friendly = plate_is_friendly(&rel, &subject, &me, is_player);
            (
                friendly,
                rel.can_attack(&me, &subject),
                rel.can_cooperate(&me, &me),
            )
        };
        let them = |template| player(OTHER, template);
        assert!(
            category(them(115), me, true).0,
            "an Alliance player is friendly"
        );
        assert!(category(them(HUMAN), me, true).0, "same race, same answer");
        assert!(!category(them(ORC), me, true).0, "a Horde player is enemy");
        // A GM-mode player, template 35, mask 0, matches no one: enemy, though not attackable.
        let (friendly, attackable, self_cooperates) = category(them(35), me, true);
        assert!(!friendly, "a GM-mode player is enemy");
        assert!(!attackable, "and not because we can attack them");
        assert!(!self_cooperates, "nobody cooperates with himself");

        // A GM-mode observer loses the friendly bucket for every player, while NPCs keep theirs:
        // `CanCooperate` is never asked for an NPC.
        let gm = player(ME, 35);
        for template in [1u32, 3, 4, 115, 2, 5, 6, 116] {
            assert!(
                !category(them(template), gm, true).0,
                "template {template} is enemy-category to a GM-mode observer",
            );
        }
        assert!(
            category(creature(NPC, 12), gm, false).0,
            "the guard keeps his bucket"
        );
        assert!(!category(creature(NPC, 14), gm, false).0, "and the mob his");
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
        use crate::target::{AttackPick, Selection};
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
                // `OBJECT_FIELD_TYPE` (2) 0x9, object and unit, as a creature's create carries it:
                // `SetSelection`'s `IsSelectable` refuses any other type.
                fields: ObjectFields::from_pairs(&[
                    (2, 0x9),
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
            crate::net::handlers::dispatch(world, crate::net::wire(vec![create(MOB)]));
            let mob = world.resource::<GuidIndex>().0[&MOB];
            *world.resource_mut::<Selection>() = Selection {
                target: Some(mob),
                guid: Some(MOB),
                ..Default::default()
            };
            // Control: a live target keeps its selection through the tracker and a ring pass.
            world
                .run_system_once(super::super::super::track::follow_selection_object)
                .expect("the tracker runs on the built client");
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
            crate::net::handlers::dispatch(world, crate::net::wire(vec![teardown]));
            assert!(
                world.get_entity(mob).is_ok(),
                "the model outlives the object"
            );
            assert!(world.get::<DespawnFade>(mob).is_some(), "…and fades");
            world
                .run_system_once(super::super::super::track::follow_selection_object)
                .expect("the tracker runs on the built client");
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
                        |mut pick: AttackPick,
                         mut sel: ResMut<Selection>,
                         mut seam: crate::creature_anim::AttackSeam,
                         errors: Res<crate::ui_action::UiErrorKeys>| {
                            pick.target(None, true, &mut sel, &mut seam, &errors)
                        },
                    )
                    .expect("the scan runs on the built client")
            };
            assert_eq!(
                acquire(world),
                Some(MOB),
                "control: the live mob is acquired"
            );
            crate::net::handlers::dispatch(
                world,
                crate::net::wire(vec![SessionEvent::ObjectsRemoved(vec![MOB])]),
            );
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
                crate::net::wire(vec![SessionEvent::ObjectDestroyed(MOB), create(MOB)]),
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
