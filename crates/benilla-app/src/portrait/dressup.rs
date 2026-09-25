//! The **dressing room** booth — the ref's `DressUpFrame`/`DressUpModel`: the
//! player's own character wearing an item they do *not* own, spun by the window's rotate buttons.
//!
//! ## Why this booth cannot be the paper doll's
//!
//! The paper doll and the inspect pane mirror a *live entity's* spawned children
//! ([`super::sync_body_booth`]) — which is exactly right for "show me what is standing in the
//! world", and exactly wrong here: nobody in the world is wearing the previewed item. So the
//! dressing room takes the **tuple-driven** path the glue screens already use — the shared
//! assembly in [`crate::entities::attach`] builds the parts from a spec
//! (body displayId + appearance dials + a 19-slot equipment array), and this module bakes them.
//!
//! The spec's equipment is the player's own visible items with the tried-on ones substituted in
//! ([`crate::ui_dressup`] composes it). One consequence worth naming: the preview dresses by the
//! **select-screen** law (weapons drawn in the hands, `0x47a0c0`), not by the
//! world's sheath state — the reference's `DressUpModel` is a `<PlayerModel>` subclass showing the
//! character posed for inspection, and that is the pose the shared assembly produces.
//!
//! ## The light is the reference's own
//!
//! `DressUpModel` (`0x495c00`) subclasses the very `CharacterModelBase` ctor (`0x505680`) the
//! character window's `<PlayerModel>` uses — same single directional light, same ambient.
//! So this bake lights through [`BoothLight::pane`], the same rig as the paper
//! doll, with no glow ([`benilla_world::ffx_glow::FfxGlow::UI_PANE`]) for the same reason.

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

/// The dressing-room booth slot token (its key in [`PortraitImages`] / [`Booths`]).
pub(crate) const DRESSUP_SLOT: &str = "dressup";

/// The character the dressing room shows: the player's own body + appearance, wearing the
/// equipment array [`crate::ui_dressup`] composed (their visible items, with each tried-on item
/// substituted into its slot).
///
/// Its `PartialEq` is the re-assembly trigger — a try-on, a reset, or the player equipping
/// something while the window is open all show up as a different look.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct DressUpLook {
    /// The player's own body display id (the ref's `SetUnit("player")`).
    pub(crate) display_id: u32,
    pub(crate) race: u8,
    pub(crate) sex: u8,
    pub(crate) skin: u8,
    pub(crate) face: u8,
    pub(crate) hair_style: u8,
    pub(crate) hair_color: u8,
    pub(crate) facial_hair: u8,
    /// Worn **ItemDisplayInfo** ids by equipment slot, the `SMSG_CHAR_ENUM` shape the shared
    /// assembly reads (helm 0 · shoulder 2 · … · main hand 15 · off hand 16 · ranged 17 · tabard 18).
    pub(crate) equipment: [CharEnumItem; 19],
    /// The player's own guild tabard — so a tried-on Guild Tabard previews *their*
    /// crest, not the blank default. Part of the look's `PartialEq`, so an identity that lands
    /// while the window is open re-assembles the booth exactly as a try-on does.
    pub(crate) emblem: Option<benilla_formats::GuildEmblem>,
}

/// The dressing room's live input: who is standing in it (`None` = the window is closed / has
/// nothing to show, which empties the booth) and the pane's bake **yaw** in radians — the ref's
/// `Model:SetRotation`, written by the window's rotate buttons exactly as the paper doll's is.
#[derive(Resource)]
pub(crate) struct DressUpPreview {
    pub(crate) look: Option<DressUpLook>,
    pub(crate) yaw: f32,
}

impl Default for DressUpPreview {
    fn default() -> Self {
        Self {
            look: None,
            // The ref's `Model_OnLoad` default facing (`UIParent.lua:1422`), same as the paper doll's.
            yaw: 0.61,
        }
    }
}

/// The assembled dressing-room look — the entities-side builder's output, the [`super::GluePreviewBake`]
/// twin. `revision` bumps on every real change (a fresh assembly, or a clear to `look: None`); the
/// booth re-bakes only when it moves, and a bare yaw change never touches it.
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

/// Stand the dressing-room booth up beside the two body panes (called from [`super::setup_booths`]).
/// Same off-screen pipeline as those — [`PAPERDOLL_SIZE`]² target, decode but no glow (decision
/// 0638) — on its own layer, framed per-bake, and **transparent** (see the clear below).
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
            // **Transparent**, unlike the paper doll's opaque near-black slab. The
            // reference's `<DressUpModel>` is a widget that draws only its model: the dark room
            // behind the character is the window's own `DressUpBackground-<Race>` art, and an
            // opaque bake hid it completely — the pane's rect covers all but a 32 px strip of it.
            //
            // 1070 lowered this quad to BACKGROUND, which is what un-covered the Reset/Close
            // buttons; it cannot un-cover the race art, because that art is on the WINDOW (level 0)
            // and the level term outranks the layer. Only compositing can. The glue booth
            // has done exactly this since 0818 — FfxGlow's combine carries the scene alpha through.
            //
            // The other three panes keep their opaque near-black: nothing is behind them to reveal,
            // and their look is the director's approved one.
            clear_color: ClearColorConfig::Custom(Color::NONE),
            ..default()
        },
        bevy::camera::RenderTarget::Image(image.clone().into()),
        benilla_world::ffx_glow::FfxGlow::UI_PANE,
        // Placeholder — `sync_dressup_booth` overwrites transform + projection from the body's
        // bounds on the first bake, exactly as the paper doll's does.
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

/// Bake the assembled dressing-room look, and spin it to the pane's yaw.
///
/// The [`super::sync_body_booth`] law over tuple-driven parts: re-light onto the pane rig, pose a
/// fresh instance, seat the riders and effects, frame it full-body, arm the wake window. What
/// differs from the paper doll is only where the parts come from (a bake resource, not a live
/// unit's children) and the hand grip — the assembly holds the weapons, so the hands close on them
/// (`CloseHand 0x479660`).
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
    // Whether a bake is currently STANDING on the stage — the empty arm's "is there anything to
    // tear down?" gate. `Booth::baked` can't answer it here: that field keys the mirrored booths'
    // look, and this one is revision-keyed.
    mut staged: Local<bool>,
) {
    if super::test_mode(&mut env_cache) {
        return; // the test bake owns the booths
    }
    let Some(booth) = booths.0.get_mut(DRESSUP_SLOT) else {
        return;
    };
    // The destination pane's aspect, latched while it is on screen — the dressing
    // room's is 316×351, so baking square made every character 11% too tall.
    let aspect = panes.0.get(DRESSUP_SLOT).copied().unwrap_or(booth.aspect);
    let (last_rev, last_yaw) = last.unwrap_or((u64::MAX, f32::NAN));
    let rebake = last_rev != bake.revision || booth.aspect != aspect;
    if !rebake && last_yaw == preview.yaw {
        return;
    }
    booth.aspect = aspect;

    if rebake {
        // Nothing to show (the window is closed, or the assembly cleared) → empty the stage and
        // let the camera sleep once it has rendered the emptied frame. Only when
        // something WAS standing here: at startup this arm runs with an empty stage already, and
        // arming the wake there paid four full booth passes to re-render nothing (caught in the
        // 1060 probe's own `[booth] t=0.00 dressup active=true wake=4` line).
        if bake.parts.is_empty() {
            if *staged {
                commands.entity(booth.root).despawn_related::<Children>();
                booth.baked = None;
                booth.wake = BOOTH_SETTLE_FRAMES;
                booth.live = false;
                booth.pending.clear();
                // The despawn reaped meshes and anchors; the rig state on the ROOT needs its
                // own strip ([`super::clear_booth_rig`]).
                super::clear_booth_rig(&mut commands, booth.root);
                booth.rigged = false;
                booth.parked = false;
                *staged = false;
            }
            *last = Some((bake.revision, preview.yaw));
            return;
        }
        // The rig + framing come from the display cache — the same readiness the assembly gated
        // on, so both are ready together. If somehow not, leave `last` alone and retry next frame.
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
                // `None` — the same known gap as the glue preview's.
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
        // Never latch a world-lane material into a pane (the [`super::light`] law): retry instead.
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
            // Stand LOOPING, like the paper doll beside it — the reference's `<DressUpModel>` is a
            // live-rendering widget and the director called the pose.
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
        // The bake animates, so its camera can't sleep — `gate_booth_cameras` runs it every frame
        // the window is drawing this pane, and none once it closes.
        // The turn's node bookkeeping named the player this bake just replaced ([`Turn::rebaked`]).
        booth.turn.rebaked();
        booth.live = true;
        // A fresh bake is animated by construction; the park state is the new rig's.
        booth.rigged = booth_rig.rigged();
        booth_rig.finish(&mut commands);
        booth.parked = false;
        *staged = true;
        aim(&mut cams, DRESSUP_SLOT, &body_frame(&anchors, aspect));
        // `WOW_BOOTH_LOG=1` — one line per committed bake, the same instrument the mirrored booths
        // carry (`super::log_bake`, whose signature is the mirrored part types'). It is what
        // separates "the pane is black because nothing baked" from "…because the camera is wrong".
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
    // The yaw (the ref's `Model:SetRotation`) — applied on a fresh bake and on every spin, never on
    // an idle frame. A spin is a content edge too.
    //
    // And the other half of `SetRotation`: the turn-in-place shuffle
    // ([`super::booth::drive_booth_turn`], 1559). The dressing room wires the same held-arrow
    // `OnUpdate` the character window does (the stock `Model_OnUpdate`, 1969), so it steps its feet
    // the same way. Keyed on the yaw alone — this block also runs for a re-bake, which is a
    // `RefreshUnit` in the reference and does not turn the model.
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
