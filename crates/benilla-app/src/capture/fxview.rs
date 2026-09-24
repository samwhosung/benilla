//! The `fxview` fixture's **driver** — the half of the effect-viewer instrument that stands the
//! subject up in the world (the request, its knobs and the camera are [`super`]'s). It lives here
//! because a fixture's driver belongs with its fixture: `entities::spell_fx` is gameplay, and
//! gameplay never depends on the capture harness.

use bevy::prelude::*;

use benilla_assets::m2_url;
use benilla_assets::materials::WowModelMaterial;

use super::{FxViewRequest, FxViewState, FXVIEW_POS};
use crate::creature_anim::FxStage;
use crate::entities::display::{empty_shell, ModelHandle};
use crate::entities::spell_fx::{
    attach_effect_visuals, EffectHost, FxMaterials, FxTintAnims, SpellFx, FALLBACK_SPAN,
};

/// The `fxview` unit lane's synthetic guid (`WOW_FX_DISPLAY`) — a high-word `0xF130` creature guid
/// like the wire's, in a serverless capture where nothing else claims one.
const FXVIEW_UNIT_GUID: u64 = (0xF130u64 << 48) | 0xFC0FEE;

/// The `fxview` GameObject lane's synthetic guid (`WOW_FX_GO`) — the wire's `0xF110` high word,
/// which is what `crate::net` reads a GO's identity out of.
const FXVIEW_GO_GUID: u64 = (0xF110u64 << 48) | 0xFC0FEE;

/// Drive the `fxview` capture fixture (see [`super::FxViewRequest`]) — the effect-
/// viewer instrument: once the capture driver arms it (scene settled), spawn a root at the
/// fixture point, create the model's [`SpellFx`] cache entry, attach the full visual set
/// through the same [`attach_effect_visuals`] body the game uses, and (for missiles) fly the
/// root along its facing so trails extend. Inert outside fxview captures (the request resource
/// only exists then).
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
        // `WOW_FX_DISPLAY`: the UNIT lane. Spawn the live component set a streamed creature gets
        // (`net::apply`'s, the same one the `vplates` fixture's wolf uses) and let the ordinary
        // unit pipeline build the model — so what this shoots is a creature, not a model, and
        // every unit-only term (tag alpha, the fade gate, anim LOD, the emitters' sequence host)
        // is in the picture. Always seated on the terrain: a creature stands on the ground.
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
                            (35, 35),  // UNIT_FIELD_FACTIONTEMPLATE — friendly, so no combat pose
                        ],
                    )),
                    Transform::from_translation(pos)
                        .with_rotation(Quat::from_rotation_y(req.yaw_deg.to_radians())),
                    Visibility::default(),
                ))
                .id();
        }
        // `WOW_FX_GO`: the GAMEOBJECT lane. A placed trap/door/chest reaches the screen through
        // `crate::go_anim`'s state machine (`0x5f3cb0`), never through the effect pool, so this is
        // the only lane that reproduces one. The descriptor carries the three fields the machine
        // reads — display, TYPE_ID (the `go_animates` gate) and STATE (the substate) — and
        // nothing else, so an unset knob renders exactly what an omitted wire field renders.
        // Always seated on the terrain: a GO stands on the ground.
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
        // `WOW_FX_GROUND=1`: seat the fixture ON the terrain — a ground-anchored effect's flat
        // quads render as projected surface decals and need ground inside their vertical slab.
        // The scene settled before arming, so the streamed tile's collider is there to hit.
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
    // A missile only trails in motion: fly the root along its model-forward (local −Z).
    // WOW_FX_TURN keeps the fixture rotating — the attached-model heading-since-birth fan.
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
    // The one-pass reap (the game's completion callback, `fx_attach`): a discrete kit
    // instance dies at exactly one pass of sequence 0 and its emitters DRAIN — the fixture
    // mirrors it so a capture past the span shows the truth. `WOW_FX_HOLD=1` previews a
    // persistent hold instead (reaped by its spell edge in game, so no clock here).
    // The one-pass reap is an effect-instance rule; a unit — or a placed GameObject — stands there.
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
        // The unit and GameObject lanes have no attach step — the entity pipeline builds the model
        // on its own. Their "age" is therefore seconds since the entity appeared, which is what the
        // animation phase is measured in.
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
            true, // the fixture plants at a world point — ground quads decal onto the terrain
            // The fixture previews kit effects, which are attached models — but it hangs on
            // nothing (there is no host model in a preview), so its pool keeps the drain.
            EffectHost { parent: None },
            // `WOW_FX_HOLD=1` previews a PERSISTENT instance, so it must show the persistent
            // lifecycle — birth then `Hold` — or the instrument would report a freeze the game
            // does not have. Without it the fixture previews a one-shot, which runs its birth once
            // and is reaped by the fixture's own span clock below.
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
            None, // the fixture previews a kit effect on its model's own `Stand`
        ) {
            state.attached_at = Some(time.elapsed_secs());
        }
    }
}
