//! The `WOW_PORTRAIT_TEST` debug bake — the booth pipeline's server-less eyeball harness
//! (`WOW_PORTRAIT_TEST=<Model\\Path.mdx>` + optional `WOW_PORTRAIT_TEST_SKIN=<blp>`): bake that
//! model into every slot (portraits + the paper doll's body framing) and own the booths (the live
//! syncs and the 0540 demand gate both stand down). Split from `mod.rs` — the harness concern,
//! not the live bake path.

use benilla_assets::M2Model;
use bevy::prelude::*;

use super::booth::{spawn_booth_model, BoothInstance, BoothMotion, BoothPart, BoothTwins};
use super::framing::{body_frame, frame, head_anchor, PortraitAnchors};
use super::{aim, test_mode, BoothCam, BoothLight, Booths, PaperDollBooth, PAPERDOLL_SLOT, SLOTS};
use benilla_assets::m2_url;

/// The debug bake driver: when `WOW_PORTRAIT_TEST` is set, bake the named model into every slot once
/// it loads, and own the booths (the live sync yields). See [`bake_test`].
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

/// The debug bake: load the env model once, then spawn its submeshes (real WowModelMaterial, untextured
/// → the muted fallback) into every slot and frame each camera. A pipeline eyeball only — no skins, no
/// cache. Returns `true` once the model has loaded + a light buffer exists and it's baked (the caller
/// then stops re-baking).
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
    // Bake only once the asset lands and the studio-light buffer exists (the material needs it).
    let Some(model) = m2s.get(&handle) else {
        return false;
    };
    // The portraits' studio light and the body pane's own — the harness bakes each
    // slot against the light that slot really uses, so the eyeball shows what ships.
    let (Some(studio), Some(pane)) = (
        booth_light.studio.buffer.clone(),
        booth_light.pane.buffer.clone(),
    ) else {
        return false;
    };
    // Optional real skin for the test bake (WOW_PORTRAIT_TEST_SKIN=<blp path>) — an untextured model
    // reads dark brown by design (the muted fallback is a gamma-dark albedo), so brightness parity
    // with the world is only judgeable textured.
    let skin: Option<Handle<Image>> = std::env::var("WOW_PORTRAIT_TEST_SKIN")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|p| {
            asset_server.load(format!(
                "mpq://{}",
                p.replace('\\', "/").to_ascii_lowercase()
            ))
        });
    // The test model's render forms, built NOW — a dev harness bake, one model.
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
                    // The booth look: the slot's own light buffer, sky lane (never ground-shaded),
                    // frozen at t = 0 — so no UV scroll and the tint seeded at its first key.
                    material: mats.off_world(
                        s,
                        s.texture.clone().or_else(|| skin.clone()),
                        u16::try_from(pi + 1).unwrap_or(u16::MAX),
                        light,
                        false,
                    ),
                    // The harness parses submeshes straight off the model, so the authored alpha is
                    // right here — an eyeball bake should show the batch dimming the artist wrote.
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
            &[],            // …nor an eye-glow
            BoothInstance::default(),
        )
        .finish(commands);
        aim(cams, token, &rig);
    }
    // Also drive the paper-doll booth from the same model, so `WOW_PORTRAIT_TEST` eyeballs the
    // full-body framing (feet/crown crop) server-less. Same all-submesh caveat as the portraits
    // (no geoset filter — a character bakes stacked hair); the live pane mirrors the filtered
    // player. Spun to the default yaw so the still reads three-quarter like the pane's default.
    if let Some(booth) = booths.0.get(PAPERDOLL_SLOT) {
        commands.entity(booth.root).despawn_related::<Children>();
        spawn_booth_model(
            commands,
            palettes,
            booth.root,
            booth.layer.clone(),
            &pane_parts, // the pane's own light, not the portraits' studio (decision 0638)
            &[],
            Some((
                &model.skeleton,
                &model.inverse_bindposes,
                model.animations.as_ref(),
            )),
            catalog,
            BoothMotion::Frozen,
            [false, false], // the paper-doll still sheaths its weapons — no in-hand grip
            &[],            // eye-glow in the paper doll is the same follow-up (see above)
            BoothInstance::default(),
        )
        .finish(commands);
        // The eyeball harness has no live UI publishing a pane, so it bakes square.
        aim(cams, PAPERDOLL_SLOT, &body_frame(&anchors, 1.0));
        commands
            .entity(booth.root)
            .insert(Transform::from_rotation(Quat::from_rotation_y(
                PaperDollBooth::default().yaw,
            )));
    }
    true
}

/// `WOW_BOOTH_DUMP=<token>:<path>:<secs>`: once `secs` of app time have elapsed, screenshot the
/// named booth's render target (e.g. `paperdoll`) to `path`. A probe run can then look at the
/// pane a live session would see under the character window — without a UI click path (the
/// first-login black-pane hunt). One shot per run; inert without the env.
///
/// **It must WAKE the booth before it shoots, and that is the whole subtlety.** Bevy's
/// `Screenshot::image` does not read the target texture's existing contents: it substitutes a
/// fresh `screenshot-capture-rendertarget` as that target's output attachment and hands back
/// whatever is *rendered into it during the capture frame* (`bevy_render`'s
/// `prepare_screenshots`). Under the 0540 demand gate a settled booth's camera is inactive and
/// its target simply "keeps the last render" — so shooting it while it sleeps renders nothing
/// into the substituted attachment and the PNG comes back a uniform `RGBA(0,0,0,0)`, which is
/// exactly the `Image::new_fill` pattern in `new_target_image` and exactly what this
/// instrument produced for its entire first life (every leg of the B106 hunt, control included,
/// byte-identical). So: arm `Booth::wake` first, hold it armed, and take the shot a few frames
/// later while the camera is still rendering. A dump that ever comes back uniformly transparent
/// again means the wake is not reaching the gate — treat it as a broken instrument, not a black
/// pane.
pub(super) fn dump_booth_target(
    mut commands: Commands,
    mut booths: ResMut<Booths>,
    time: Res<Time<bevy::time::Real>>,
    mut phase: Local<u32>,
) {
    /// Frames to hold the booth awake before taking the shot: the gate reads `wake` and flips
    /// `Camera::is_active` in the same frame, so one would do — a small margin covers the
    /// command-applied camera flip and the render-app extract behind it.
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
    // Hold the gate open across the lead AND the capture frame itself.
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

/// Turn a booth render-target readback into something the PNG encoder can write — and, more to the
/// point, into what the **screen** shows.
///
/// The targets are `Rgba16Float` holding **un-encoded** values (`super::new_target_image`): the
/// display encode is the UI arc's, applied when the glue/paper-doll tree samples the target
/// (`crate::ui_gamma`), and a readback bypasses that lane entirely. So it happens here. Without it
/// the PNG is ~2.2× dark — which is precisely what this instrument produced for its whole first life
/// (it relabeled the un-encoded 8-bit bytes as sRGB and saved them verbatim), and a dump that reads
/// darker than the screen is a debugging instrument that lies about the thing it exists to show.
///
/// `None` on an unexpected format, so a future target-format change is a loud warning rather than a
/// garbled PNG.
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
            // The sRGB transfer function for colour (channel 3 is plain coverage, never encoded) —
            // the same curve the swapchain's `…Srgb` write applies to the live frame.
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

    /// The dump encodes colour and passes alpha through — the screen's transfer function, applied
    /// where the readback bypassed the UI lane that would have applied it.
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
        // 0.5 linear is display 188 — NOT 128. That gap is the whole reason this exists: for its
        // first life the instrument wrote the 128.
        assert_eq!(bytes[0], 188);
        // The old 8-bit linear target's second code lands on 13 — the bottom of the ladder B126
        // measured, and the reason a float target has ~4× the levels down here.
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
