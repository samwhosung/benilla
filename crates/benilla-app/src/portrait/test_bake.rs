//! The `WOW_PORTRAIT_TEST=<Model\\Path.mdx>` debug bake (optional
//! `WOW_PORTRAIT_TEST_SKIN=<blp>`): bakes that model into every booth slot with no server and owns
//! the booths, so the live syncs and the demand gate stand down.

use benilla_assets::M2Model;
use bevy::prelude::*;

use super::booth::{spawn_booth_model, BoothInstance, BoothMotion, BoothPart, BoothTwins};
use super::framing::{body_frame, frame, head_anchor, PortraitAnchors};
use super::{aim, test_mode, BoothCam, BoothLight, Booths, PaperDollBooth, PAPERDOLL_SLOT, SLOTS};
use benilla_assets::m2_url;

/// When `WOW_PORTRAIT_TEST` is set, bake the named model into every slot once it loads.
pub(super) fn sync_test_portraits(
    mut commands: Commands,
    booths: Res<Booths>,
    booth_light: Res<BoothLight>,
    m2s: Res<Assets<M2Model>>,
    asset_server: Res<AssetServer>,
    mut mats: benilla_world::model_render::M2BatchMaterials,
    mut test_handle: Local<Option<Handle<M2Model>>>,
    mut test_done: Local<bool>,
    mut env_cache: Local<Option<bool>>,
    mut cams: Query<(&BoothCam, &mut Transform, &mut Projection)>,
    anim_data: Option<Res<crate::creature_anim::AnimData>>,
    mut palettes: ResMut<benilla_world::rig_palette::RigPalettes>,
    mut forms: ResMut<benilla_world::model_forms::ModelForms>,
    mut mesh_assets: ResMut<Assets<Mesh>>,
) {
    if !test_mode(&mut env_cache) || *test_done {
        return;
    }
    let path = std::env::var("WOW_PORTRAIT_TEST").expect("gated by test_mode");
    if bake_test(
        &mut commands,
        &mut palettes,
        &booths,
        &path,
        &asset_server,
        &m2s,
        &mut forms,
        &mut mesh_assets,
        &mut mats,
        &booth_light,
        &mut test_handle,
        &mut cams,
        anim_data.as_deref().map(|a| &a.0),
    ) {
        *test_done = true;
    }
}

/// Load the env model once, spawn its submeshes into every slot and frame each camera; `true` once
/// baked. All submeshes, no geoset filter and no cache: an untextured part gets the muted fallback.
fn bake_test(
    commands: &mut Commands,
    palettes: &mut benilla_world::rig_palette::RigPalettes,
    booths: &Booths,
    path: &str,
    asset_server: &AssetServer,
    m2s: &Assets<M2Model>,
    forms: &mut benilla_world::model_forms::ModelForms,
    mesh_assets: &mut Assets<Mesh>,
    mats: &mut benilla_world::model_render::M2BatchMaterials,
    booth_light: &BoothLight,
    test_handle: &mut Option<Handle<M2Model>>,
    cams: &mut Query<(&BoothCam, &mut Transform, &mut Projection)>,
    catalog: Option<&benilla_formats::AnimDataCatalog>,
) -> bool {
    let handle = test_handle
        .get_or_insert_with(|| asset_server.load(m2_url(path)))
        .clone();
    // Bake only once the asset lands and the light buffers exist.
    let Some(model) = m2s.get(&handle) else {
        return false;
    };
    // Each slot bakes against the light it really uses: the portraits' studio, the pane's own.
    let (Some(studio), Some(pane)) = (
        booth_light.studio.buffer.clone(),
        booth_light.pane.buffer.clone(),
    ) else {
        return false;
    };
    // An untextured model reads dark brown (the muted fallback), so brightness is only judgeable
    // with a real skin.
    let skin: Option<Handle<Image>> = std::env::var("WOW_PORTRAIT_TEST_SKIN")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|p| {
            asset_server.load(format!(
                "mpq://{}",
                p.replace('\\', "/").to_ascii_lowercase()
            ))
        });
    // Built now: a harness bake of one model.
    forms.ensure_now_rigged(&handle, &model.submeshes, mesh_assets);
    let built = forms.slices(&handle);
    let (stat_forms, skin_forms) = (built.stat, built.skin.unwrap_or(&[]));
    let parts_against =
        |light: &bevy::render::render_resource::Buffer,
         mats: &mut benilla_world::model_render::M2BatchMaterials| {
            model
                .submeshes
                .iter()
                .enumerate()
                .map(|(pi, s)| BoothPart {
                    skinned: skin_forms.get(pi).cloned(),
                    static_mesh: stat_forms
                        .get(pi)
                        .map(|(h, _)| h.clone())
                        .unwrap_or_default(),
                    // The slot's light buffer, sky lane, frozen at t = 0: no UV scroll, the tint
                    // at its first key.
                    material: mats.off_world(
                        s,
                        s.texture.clone().or_else(|| skin.clone()),
                        u16::try_from(pi + 1).unwrap_or(u16::MAX),
                        light,
                        false,
                    ),
                    // Straight off the model, so the authored alpha animation is at hand.
                    alpha_anim: s.alpha_anim.clone(),
                    twins: BoothTwins::default(),
                    mat_anim: false,
                })
                .collect::<Vec<BoothPart>>()
        };
    let parts = parts_against(&studio, mats);
    let pane_parts = parts_against(&pane, mats);
    let (pivot_height, ground_radius) = model
        .bounds
        .map(|b| (b.pivot_z.map_or(0.0, |z| z + 0.0972), b.ring_footprint))
        .unwrap_or((0.0, 0.0));
    let anchors = PortraitAnchors {
        camera: model.portrait_camera,
        pane_camera: model.pane_camera,
        bbox_center: model.bounds.map_or(Vec3::ZERO, |b| {
            benilla_assets::coords::wow_to_bevy([
                (b.bbox_min[0] + b.bbox_max[0]) * 0.5,
                (b.bbox_min[1] + b.bbox_max[1]) * 0.5,
                (b.bbox_min[2] + b.bbox_max[2]) * 0.5,
            ])
        }),
        head: head_anchor(&model.skeleton, &model.attachments),
        pivot_height,
        ground_radius,
    };
    let rig = frame(&anchors);
    match anchors.camera {
        Some(c) => info!(
            "portrait test bake: {} submeshes, AUTHORED camera eye={:?} target={:?} fov={:.3} near={:.3} far={:.1}",
            model.submeshes.len(), c.eye, c.target, c.fov, c.near, c.far
        ),
        None => info!(
            "portrait test bake: {} submeshes, NO authored camera — heuristic head={:?} pivot={pivot_height:.2} foot={ground_radius:.2}",
            model.submeshes.len(), anchors.head
        ),
    }
    for token in SLOTS {
        let Some(booth) = booths.0.get(token) else {
            continue;
        };
        commands.entity(booth.root).despawn_related::<Children>();
        spawn_booth_model(
            commands,
            palettes,
            booth.root,
            booth.layer.clone(),
            &parts,
            &[], // the test bake dresses no riders
            Some((
                &model.skeleton,
                &model.inverse_bindposes,
                model.animations.as_ref(),
            )),
            catalog,
            BoothMotion::Frozen,
            [false, false], // the WOW_PORTRAIT_TEST bake dresses no weapons
            &[],            // nor an eye-glow
            BoothInstance::default(),
        )
        .finish(commands);
        aim(cams, token, &rig);
    }
    // The paper-doll booth too, to check the full-body framing, spun to the pane's default yaw.
    if let Some(booth) = booths.0.get(PAPERDOLL_SLOT) {
        commands.entity(booth.root).despawn_related::<Children>();
        spawn_booth_model(
            commands,
            palettes,
            booth.root,
            booth.layer.clone(),
            &pane_parts, // the pane's own light, not the portraits' studio
            &[],
            Some((
                &model.skeleton,
                &model.inverse_bindposes,
                model.animations.as_ref(),
            )),
            catalog,
            BoothMotion::Frozen,
            [false, false], // the harness bake holds no weapons, so no in-hand grip
            &[],            // no eye-glow
            BoothInstance::default(),
        )
        .finish(commands);
        // No live UI publishes a pane aspect here, so it bakes square.
        aim(cams, PAPERDOLL_SLOT, &body_frame(&anchors, 1.0));
        commands
            .entity(booth.root)
            .insert(Transform::from_rotation(Quat::from_rotation_y(
                PaperDollBooth::default().yaw,
            )));
    }
    true
}

/// `WOW_BOOTH_DUMP=<token>:<path>:<secs>`: after `secs` of app time, save the named booth's render
/// target (e.g. `paperdoll`) to `path` as a PNG, once; inert without the env.
///
/// It must wake the booth first: `Screenshot::image` returns only what renders during the capture
/// frame (`bevy_render`'s `prepare_screenshots`), and a settled booth's camera is inactive, so a
/// sleeping shot comes back uniform `RGBA(0,0,0,0)`. A uniformly transparent dump means the wake
/// is not reaching the gate, not a black pane.
pub(super) fn dump_booth_target(
    mut commands: Commands,
    mut booths: ResMut<Booths>,
    time: Res<Time<bevy::time::Real>>,
    mut phase: Local<u32>,
) {
    /// Frames awake before the shot: one would do, the margin covers the deferred camera flip and
    /// the render-app extract.
    const WAKE_LEAD: u32 = 3;
    const DONE: u32 = u32::MAX;

    static SPEC: std::sync::OnceLock<Option<(String, String, f32)>> = std::sync::OnceLock::new();
    let Some((token, path, secs)) = SPEC.get_or_init(|| {
        let v = std::env::var("WOW_BOOTH_DUMP").ok()?;
        let mut it = v.splitn(3, ':');
        Some((
            it.next()?.to_string(),
            it.next()?.to_string(),
            it.next()?.parse().ok()?,
        ))
    }) else {
        return;
    };
    if *phase == DONE || (*phase == 0 && time.elapsed_secs() < *secs) {
        return;
    }
    let Some(booth) = booths.0.get_mut(token.as_str()) else {
        warn!("WOW_BOOTH_DUMP: no booth named {token:?}");
        *phase = DONE;
        return;
    };
    // Hold the gate open across the lead and the capture frame.
    booth.wake = booth.wake.max(super::BOOTH_SETTLE_FRAMES);
    if *phase < WAKE_LEAD {
        if *phase == 0 {
            info!("WOW_BOOTH_DUMP: waking booth {token:?} for the shot");
        }
        *phase += 1;
        return;
    }
    *phase = DONE;
    use bevy::render::view::window::screenshot::{Screenshot, ScreenshotCaptured};
    info!("WOW_BOOTH_DUMP: shooting booth {token:?} -> {path}");
    let out = std::path::PathBuf::from(path.clone());
    commands
        .spawn(Screenshot::image(booth.target.clone()))
        .observe(move |shot: On<ScreenshotCaptured>| {
            let Some(img) = encode_target_readback(&shot.image) else {
                warn!("WOW_BOOTH_DUMP: unexpected target format, nothing saved");
                return;
            };
            match img.try_into_dynamic() {
                Ok(dyn_img) => match dyn_img.save(&out) {
                    Ok(()) => info!("WOW_BOOTH_DUMP: saved {}", out.display()),
                    Err(e) => warn!("WOW_BOOTH_DUMP: save failed: {e}"),
                },
                Err(e) => warn!("WOW_BOOTH_DUMP: convert failed: {e}"),
            }
        });
}

/// Encode a booth readback as the screen shows it. The targets are `Rgba16Float` holding
/// un-encoded values (the display encode happens where the UI samples them, `crate::ui_gamma`), so
/// a readback needs the sRGB curve here or the PNG is about 2.2× dark. `None` on any other format.
pub(crate) fn encode_target_readback(shot: &Image) -> Option<Image> {
    use bevy::asset::RenderAssetUsages;
    use bevy::render::render_resource::{TextureDimension, TextureFormat};

    if shot.texture_descriptor.format != TextureFormat::Rgba16Float {
        return None;
    }
    let src = shot.data.as_ref()?;
    let mut out = Vec::with_capacity(src.len() / 2);
    for texel in src.as_chunks::<8>().0 {
        for (c, half_pair) in texel.as_chunks::<2>().0.iter().enumerate() {
            let v = half::f16::from_le_bytes([half_pair[0], half_pair[1]]).to_f32();
            // The sRGB transfer function, as the swapchain's `Srgb` write applies; channel 3 is
            // coverage, never encoded.
            let encoded = match c {
                3 => v,
                _ if v <= 0.003_130_8 => v * 12.92,
                _ => 1.055 * v.powf(1.0 / 2.4) - 0.055,
            };
            out.push((encoded.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }
    Some(Image::new(
        shot.texture_descriptor.size,
        TextureDimension::D2,
        out,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::RenderAssetUsages;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

    fn readback(format: TextureFormat, data: Vec<u8>) -> Image {
        Image::new(
            Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            data,
            format,
            RenderAssetUsages::default(),
        )
    }

    /// The dump encodes colour with the screen's transfer function and passes alpha through.
    #[test]
    fn the_booth_dump_encodes_colour_and_leaves_alpha_alone() {
        let texel = |v: f32| half::f16::from_f32(v).to_le_bytes().to_vec();
        let mut data = Vec::new();
        // Mid grey, then the darkest step an 8-bit LINEAR target could hold, then opaque alpha.
        data.extend(texel(0.5));
        data.extend(texel(1.0 / 255.0));
        data.extend(texel(0.0));
        data.extend(texel(1.0));

        let out = encode_target_readback(&readback(TextureFormat::Rgba16Float, data))
            .expect("float readback encodes");
        let bytes = out.data.as_ref().expect("encoded bytes");
        // 0.5 linear is display 188, not 128.
        assert_eq!(bytes[0], 188);
        // An 8-bit linear target's second code lands on 13: a float target has more levels here.
        assert_eq!(bytes[1], 13);
        assert_eq!(bytes[2], 0);
        // Alpha is coverage, never encoded.
        assert_eq!(bytes[3], 255);
    }

    /// A format the encoder doesn't know is a loud `None`, not a garbled PNG.
    #[test]
    fn an_unexpected_readback_format_refuses() {
        let img = readback(TextureFormat::Rgba8Unorm, vec![1, 2, 3, 4]);
        assert!(encode_target_readback(&img).is_none());
    }
}
