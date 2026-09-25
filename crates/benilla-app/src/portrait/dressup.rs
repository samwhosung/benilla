//! The dressing-room booth, the reference's `DressUpFrame`/`DressUpModel`: the player's own
//! character wearing items they do not own. Nobody in the world wears the preview, so it is built
//! from a spec by the shared assembly ([`crate::entities::attach`]), not mirrored from a live
//! entity, and it dresses by the select-screen law (weapons in hand, `0x47a0c0`). `DressUpModel`
//! (`0x495c00`) shares the `CharacterModelBase` ctor (`0x505680`) with the character window's
//! `<PlayerModel>`, so it lights through [`BoothLight::pane`] with no glow, as the paper doll does.

use benilla_protocol::CharEnumItem;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::PerspectiveProjection;
use bevy::prelude::*;

use crate::entities::Creatures;
use benilla_assets::materials::WowModelMaterial;

use super::light::BoothLight;
use super::{
    aim, body_frame, booth_anchors, new_target_image, spawn_booth_effects, spawn_booth_model,
    wake_booth, Booth, BoothBillboardSpec, BoothCam, BoothEffects, BoothInstance, BoothMotion,
    BoothPart, BoothRider, BoothTwins, Booths, PortraitImages, PortraitSource, PreviewBillboard,
    PreviewEffects, PreviewPart, PreviewRider, BOOTH_SETTLE_FRAMES, DRESSUP_LAYER, PAPERDOLL_SIZE,
};

/// The dressing-room booth's key in [`PortraitImages`] and [`Booths`].
pub(crate) const DRESSUP_SLOT: &str = "dressup";

/// The player's own body and appearance wearing [`crate::ui_dressup`]'s equipment array (their
/// visible items with the tried-on ones substituted); a change in it re-assembles the booth.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct DressUpLook {
    /// The player's own body display id (the reference's `SetUnit("player")`).
    pub(crate) display_id: u32,
    pub(crate) race: u8,
    pub(crate) sex: u8,
    pub(crate) skin: u8,
    pub(crate) face: u8,
    pub(crate) hair_style: u8,
    pub(crate) hair_color: u8,
    pub(crate) facial_hair: u8,
    /// `ItemDisplayInfo` ids by equipment slot, in the `SMSG_CHAR_ENUM` shape (helm 0, main hand
    /// 15, off hand 16, ranged 17, tabard 18).
    pub(crate) equipment: [CharEnumItem; 19],
    /// The player's own guild crest, so a tried-on Guild Tabard shows it, not the blank default.
    pub(crate) emblem: Option<benilla_formats::GuildEmblem>,
}

/// The dressing room's input: the look (`None` empties the booth) and the yaw in radians, the
/// reference's `Model:SetRotation` driven by the rotate buttons.
#[derive(Resource)]
pub(crate) struct DressUpPreview {
    pub(crate) look: Option<DressUpLook>,
    pub(crate) yaw: f32,
}

impl Default for DressUpPreview {
    fn default() -> Self {
        Self {
            look: None,
            // `Model_OnLoad`'s default facing (`UIParent.lua:1422`).
            yaw: 0.61,
        }
    }
}

/// The assembled dressing-room parts. `revision` bumps on a fresh assembly or a clear; the booth
/// re-bakes only when it moves, never on a bare yaw change.
#[derive(Resource, Default)]
pub(crate) struct DressUpBake {
    pub(crate) look: Option<DressUpLook>,
    pub(crate) display_id: u32,
    pub(crate) parts: Vec<PreviewPart>,
    pub(crate) riders: Vec<PreviewRider>,
    pub(crate) effects: Vec<PreviewEffects>,
    pub(crate) billboards: Vec<PreviewBillboard>,
    pub(crate) grip: [bool; 2],
    pub(crate) revision: u64,
}

/// Stand the dressing-room booth up: a [`PAPERDOLL_SIZE`]² target with no glow on its own layer,
/// framed per bake, and transparent.
pub(super) fn spawn_dressup_booth(
    commands: &mut Commands,
    images: &mut Assets<Image>,
    portraits: &mut PortraitImages,
    booths: &mut Booths,
) {
    let image = images.add(new_target_image(PAPERDOLL_SIZE));
    portraits.0.insert(
        DRESSUP_SLOT.to_string(),
        PortraitSource::Live(image.clone()),
    );
    let layer = RenderLayers::layer(DRESSUP_LAYER);
    let root = commands
        .spawn((Transform::IDENTITY, Visibility::Visible, layer.clone()))
        .id();
    commands.spawn((
        super::booth_view_shape(),
        Camera {
            order: -100 + DRESSUP_LAYER as isize,
            // Transparent: `<DressUpModel>` draws only its model, and the room behind it is the
            // window's `DressUpBackground-<Race>` art at a lower frame level, which only
            // compositing can show through.
            clear_color: ClearColorConfig::Custom(Color::NONE),
            ..default()
        },
        bevy::camera::RenderTarget::Image(image.clone().into()),
        benilla_world::ffx_glow::FfxGlow::UI_PANE,
        // Placeholder: `sync_dressup_booth` frames it from the body's bounds on the first bake.
        Projection::from(PerspectiveProjection {
            fov: super::PORTRAIT_FOV,
            near: 0.02,
            far: 100.0,
            ..default()
        }),
        layer.clone(),
        BoothCam(DRESSUP_SLOT.to_string()),
    ));
    booths.0.insert(
        DRESSUP_SLOT.to_string(),
        Booth {
            layer,
            root,
            target: image,
            baked: None,
            baked_guid: None,
            snap: None,
            shown: false,
            show_rev: 0,
            wake: 0,
            live: false,
            pending: Vec::new(),
            pending_since: None,
            pipes_settling: false,
            pipes_since: None,
            aspect: 1.0,
            rigged: false,
            parked: false,
            turn: super::Turn::default(),
        },
    );
}

/// Bake the assembled look as [`super::sync_body_booth`] does, and spin it to the pane's yaw. The
/// assembly holds the weapons, so the hands close on them (`CloseHand` `0x479660`).
pub(super) fn sync_dressup_booth(
    mut commands: Commands,
    preview: Res<DressUpPreview>,
    bake: Res<DressUpBake>,
    mut booths: ResMut<Booths>,
    panes: Res<super::BoothPanes>,
    mut booth_light: ResMut<BoothLight>,
    mut materials: ResMut<Assets<WowModelMaterial>>,
    creatures: Option<Res<Creatures>>,
    anim_data: Option<Res<crate::creature_anim::AnimData>>,
    mut cams: Query<(&BoothCam, &mut Transform, &mut Projection)>,
    mut palettes: ResMut<benilla_world::rig_palette::RigPalettes>,
    mut env_cache: Local<Option<bool>>,
    mut last: Local<Option<(u64, f32)>>,
    // Whether a bake stands on the stage; `Booth::baked` keys the mirrored booths, not this one.
    mut staged: Local<bool>,
) {
    if super::test_mode(&mut env_cache) {
        return; // the test bake owns the booths
    }
    let Some(booth) = booths.0.get_mut(DRESSUP_SLOT) else {
        return;
    };
    // The pane's aspect, latched while on screen: the dressing room's is 316×351, not square.
    let aspect = panes.0.get(DRESSUP_SLOT).copied().unwrap_or(booth.aspect);
    let (last_rev, last_yaw) = last.unwrap_or((u64::MAX, f32::NAN));
    let rebake = last_rev != bake.revision || booth.aspect != aspect;
    if !rebake && last_yaw == preview.yaw {
        return;
    }
    booth.aspect = aspect;

    if rebake {
        // Nothing to show: empty the stage, and wake only if something stood there, so an
        // already-empty stage at startup costs no booth passes.
        if bake.parts.is_empty() {
            if *staged {
                commands.entity(booth.root).despawn_related::<Children>();
                booth.baked = None;
                booth.wake = BOOTH_SETTLE_FRAMES;
                booth.live = false;
                booth.pending.clear();
                // The despawn reaped meshes and anchors; the rig state on the root needs its own.
                super::clear_booth_rig(&mut commands, booth.root);
                booth.rigged = false;
                booth.parked = false;
                *staged = false;
            }
            *last = Some((bake.revision, preview.yaw));
            return;
        }
        // Rig and framing come from the display cache the assembly gated on; if not ready,
        // leave `last` alone and retry next frame.
        let Some(creatures) = creatures.as_deref() else {
            return;
        };
        let (Some(rig), Some(anchors)) = (
            creatures.display_rig(bake.display_id),
            booth_anchors(Some(creatures), Some(bake.display_id)),
        ) else {
            booth.wake = booth.wake.max(BOOTH_SETTLE_FRAMES);
            return;
        };
        let mut relight =
            |m: &Handle<WowModelMaterial>| booth_light.pane.variant(m, &mut materials);
        let booth_parts: Vec<BoothPart> = bake
            .parts
            .iter()
            .map(|p| BoothPart {
                skinned: p.skinned_mesh.clone(),
                static_mesh: p.static_mesh.clone(),
                material: relight(&p.material),
                // Not built, as in the glue preview.
                alpha_anim: None,
                twins: BoothTwins::default(),
                mat_anim: false,
            })
            .collect();
        let booth_riders: Vec<BoothRider> = bake
            .riders
            .iter()
            .map(|r| BoothRider {
                mesh: r.mesh.clone(),
                material: relight(&r.material),
                bone: r.bone,
                offset: r.offset,
                twins: BoothTwins::default(),
            })
            .collect();
        let booth_billboards: Vec<BoothBillboardSpec> = bake
            .billboards
            .iter()
            .map(|b| BoothBillboardSpec {
                mesh: b.mesh.clone(),
                material: relight(&b.material),
                bone: b.bone,
                offset: b.offset,
                kind: b.kind,
                twins: BoothTwins::default(),
            })
            .collect();
        // Never latch a world-lane material into a pane ([`super::light`]): retry instead.
        if booth_light.pane.take_unready() {
            booth.wake = booth.wake.max(BOOTH_SETTLE_FRAMES);
            return;
        }
        commands.entity(booth.root).despawn_related::<Children>();
        let mut booth_rig = spawn_booth_model(
            &mut commands,
            &mut palettes,
            booth.root,
            booth.layer.clone(),
            &booth_parts,
            &booth_riders,
            rig.inverse_bindposes
                .as_ref()
                .map(|ibp| (rig.skeleton, ibp, rig.animations)),
            anim_data.as_deref().map(|a| &a.0),
            // Stand looping, like the paper doll: `<DressUpModel>` renders live.
            BoothMotion::Loop,
            bake.grip,
            &booth_billboards,
            BoothInstance::default(),
        );
        let (fx_emitters, _) = spawn_booth_effects(
            &mut commands,
            &mut booth_rig,
            &booth.layer,
            booth_light.pane.buffer.as_ref(),
            &bake
                .effects
                .iter()
                .map(|fx| BoothEffects {
                    bone: fx.bone,
                    offset: fx.offset,
                    emitters: fx.emitters.clone(),
                })
                .collect::<Vec<_>>(),
            BoothInstance::default(),
        );
        // The bake animates, so `gate_booth_cameras` runs its camera every frame the pane draws.
        booth.turn.rebaked();
        booth.live = true;
        // A fresh bake is animated; the park state is the new rig's.
        booth.rigged = booth_rig.rigged();
        booth_rig.finish(&mut commands);
        booth.parked = false;
        *staged = true;
        aim(&mut cams, DRESSUP_SLOT, &body_frame(&anchors, aspect));
        // `WOW_BOOTH_LOG=1`: one line per committed bake, as `super::log_bake` for the mirrored
        // booths.
        if super::booth_log() {
            eprintln!(
                "[booth] dressup bake parts={} riders={} billboards={} fx={} rev={} aspect={aspect:.3}",
                booth_parts.len(),
                booth_riders.len(),
                booth_billboards.len(),
                fx_emitters,
                bake.revision,
            );
        }
        wake_booth(
            booth,
            &materials,
            booth_parts
                .iter()
                .map(|p| &p.material)
                .chain(booth_riders.iter().map(|r| &r.material))
                .chain(booth_billboards.iter().map(|b| &b.material)),
        );
    }
    // The yaw, `Model:SetRotation`, applied on a fresh bake and on every spin. A spin also steps
    // the feet ([`super::booth::drive_booth_turn`]), as the stock `Model_OnUpdate` held-arrow
    // turn does; keyed on the yaw alone, since a re-bake is a `RefreshUnit` and does not turn.
    if booth.turn.faced != Some(preview.yaw) {
        if let Some(prev) = booth.turn.faced {
            booth.turn.spun = Some(super::booth::turn_shuffle(prev, preview.yaw));
        }
        booth.turn.faced = Some(preview.yaw);
    }
    commands
        .entity(booth.root)
        .insert(Transform::from_rotation(Quat::from_rotation_y(preview.yaw)));
    booth.wake = booth.wake.max(BOOTH_SETTLE_FRAMES);
    *last = Some((bake.revision, preview.yaw));
}
