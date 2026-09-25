//! The `fxview` fixture's driver: stands the effect-viewer's subject up in the world; the
//! request, its knobs and the camera are [`super`]'s.

use bevy::prelude::*;

use benilla_assets::m2_url;
use benilla_assets::materials::WowModelMaterial;

use super::{FxViewRequest, FxViewState, FXVIEW_POS};
use crate::creature_anim::FxStage;
use crate::entities::display::{empty_shell, ModelHandle};
use crate::entities::spell_fx::{
    attach_effect_visuals, EffectHost, FxMaterials, FxTintAnims, SpellFx, FALLBACK_SPAN,
};

/// The unit lane's synthetic guid (`WOW_FX_DISPLAY`): a wire-shaped `0xF130` creature guid.
const FXVIEW_UNIT_GUID: u64 = (0xF130u64 << 48) | 0xFC0FEE;

/// The GameObject lane's synthetic guid (`WOW_FX_GO`): the `0xF110` high word `crate::net` reads
/// a GO's identity from.
const FXVIEW_GO_GUID: u64 = (0xF110u64 << 48) | 0xFC0FEE;

/// Drives the `fxview` fixture once armed: spawns a root, attaches the model through the game's
/// own [`attach_effect_visuals`], and flies a missile along its facing so trails extend.
pub(crate) fn drive_fx_view(
    req: Option<Res<FxViewRequest>>,
    state: Option<ResMut<FxViewState>>,
    mut commands: Commands,
    fx: Option<ResMut<SpellFx>>,
    asset_server: Res<AssetServer>,
    time: Res<Time>,
    mut wow_materials: ResMut<Assets<WowModelMaterial>>,
    mut tint_reg: ResMut<FxTintAnims>,
    mut uv_reg: ResMut<benilla_world::doodad_anim::UvAnimMaterials>,
    mut anim_table: ResMut<benilla_world::mat_anim_table::MatAnimTable>,
    ibps: Res<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    mut palettes: ResMut<benilla_world::rig_palette::RigPalettes>,
    spatial: avian3d::prelude::SpatialQuery,
    mut transforms: Query<&mut Transform>,
) {
    let (Some(req), Some(mut state)) = (req, state) else {
        return;
    };
    if !state.armed {
        return;
    }
    let Some(mut fx) = fx else {
        // Server-less capture boot: the cache resource normally rides the net session.
        commands.init_resource::<SpellFx>();
        return;
    };
    let root = *state.root.get_or_insert_with(|| {
        let mut pos = benilla_assets::coords::wow_to_bevy(req.at.unwrap_or(FXVIEW_POS));
        // `WOW_FX_DISPLAY`, the unit lane: the component set `net::apply` gives a streamed
        // creature, so the unit pipeline builds it with every unit-only term; seated on terrain.
        if let Some(display) = req.display {
            if let Some(hit) = spatial.cast_ray(
                pos,
                Dir3::NEG_Y,
                500.0,
                true,
                &benilla_world::collision::WorldCollision::body_filter(),
            ) {
                pos.y -= hit.distance;
            }
            pos.y += req.up;
            return commands
                .spawn((
                    crate::net::Guid(FXVIEW_UNIT_GUID),
                    crate::net::NetEntity {
                        kind: benilla_protocol::EntityKind::Unit,
                        display_id: Some(display),
                        scale: req.scale,
                    },
                    crate::net::ObjectStore(benilla_protocol::messages::ObjectFields::from_pairs(
                        &[
                            (22, 100), // UNIT_FIELD_HEALTH
                            (28, 100), // UNIT_FIELD_MAXHEALTH
                            (34, 60),  // UNIT_FIELD_LEVEL
                            (35, 35),  // UNIT_FIELD_FACTIONTEMPLATE: friendly, no combat pose
                        ],
                    )),
                    Transform::from_translation(pos)
                        .with_rotation(Quat::from_rotation_y(req.yaw_deg.to_radians())),
                    Visibility::default(),
                ))
                .id();
        }
        // `WOW_FX_GO`, the GameObject lane: a placed GO animates through `crate::go_anim`'s state
        // machine (`0x5f3cb0`), not the effect pool. The descriptor carries only display, TYPE_ID
        // and STATE, the fields the machine reads; seated on terrain.
        if let Some(display) = req.go {
            if let Some(hit) = spatial.cast_ray(
                pos,
                Dir3::NEG_Y,
                500.0,
                true,
                &benilla_world::collision::WorldCollision::body_filter(),
            ) {
                pos.y -= hit.distance;
            }
            pos.y += req.up;
            return commands
                .spawn((
                    crate::net::Guid(FXVIEW_GO_GUID),
                    crate::net::NetEntity {
                        kind: benilla_protocol::EntityKind::GameObject,
                        display_id: Some(display),
                        scale: req.scale,
                    },
                    crate::net::ObjectStore(benilla_protocol::messages::ObjectFields::from_pairs(
                        &[
                            (14, req.go_state), // GAMEOBJECT_STATE
                            (21, req.go_type),  // GAMEOBJECT_TYPE_ID
                        ],
                    )),
                    Transform::from_translation(pos)
                        .with_rotation(Quat::from_rotation_y(req.yaw_deg.to_radians())),
                    Visibility::default(),
                ))
                .id();
        }
        // `WOW_FX_GROUND=1`: a ground-anchored effect's quads decal onto terrain inside their slab.
        if req.ground {
            if let Some(hit) = spatial.cast_ray(
                pos,
                Dir3::NEG_Y,
                500.0,
                true,
                &benilla_world::collision::WorldCollision::body_filter(),
            ) {
                pos.y -= hit.distance;
            }
        }
        pos.y += req.up;
        commands
            .spawn((
                Transform::from_translation(pos)
                    .with_rotation(Quat::from_rotation_y(req.yaw_deg.to_radians())),
                Visibility::default(),
            ))
            .id()
    });
    // A missile only trails in motion: fly along model-forward (local -Z); `WOW_FX_TURN` spins it.
    if req.fly > 0.0 || req.turn != 0.0 {
        if let Ok(mut t) = transforms.get_mut(root) {
            let fwd = t.rotation * -Vec3::Z;
            t.translation += fwd * req.fly * time.delta_secs();
            if req.turn != 0.0 {
                t.rotation =
                    Quat::from_rotation_y((req.turn * time.delta_secs()).to_radians()) * t.rotation;
            }
        }
    }
    // The game's one-pass reap (`fx_attach`): a one-shot kit instance dies after one pass of
    // sequence 0. Not for `WOW_FX_HOLD=1` (reaped by its spell in game) or a unit or GO.
    if !req.hold && !state.expired && req.display.is_none() && req.go.is_none() {
        if let Some(at) = state.attached_at {
            let span = fx
                .models
                .get(&req.model_path)
                .and_then(|dm| dm.first_seq_span)
                .unwrap_or(FALLBACK_SPAN);
            if time.elapsed_secs() >= at + span {
                commands.entity(root).despawn();
                state.expired = true;
            }
        }
    }
    if state.attached_at.is_none() {
        // The unit and GO lanes have no attach step: their age counts from the entity's spawn.
        if req.display.is_some() || req.go.is_some() {
            state.attached_at = Some(time.elapsed_secs());
            return;
        }
        fx.models.entry(req.model_path.clone()).or_insert_with(|| {
            let mut dm = empty_shell();
            dm.handle = ModelHandle::M2(asset_server.load(m2_url(&req.model_path)));
            dm
        });
        if attach_effect_visuals(
            &mut commands,
            root,
            &fx.models[&req.model_path],
            time.elapsed_secs(),
            true, // planted at a world point: ground quads decal onto the terrain
            // No host model in a preview, so the pool keeps the drain.
            EffectHost { parent: None },
            // `WOW_FX_HOLD=1` shows the persistent lifecycle (birth then `Hold`); otherwise a
            // one-shot, reaped by the span clock above.
            Some(if req.hold {
                FxStage::State
            } else {
                FxStage::OneShot
            }),
            &mut FxMaterials {
                store: &mut wow_materials,
                tint: &mut tint_reg,
                uv: &mut uv_reg,
                table: &mut anim_table,
            },
            &ibps,
            &mut palettes,
            None, // a kit effect on its model's own `Stand`
        ) {
            state.attached_at = Some(time.elapsed_secs());
        }
    }
}
