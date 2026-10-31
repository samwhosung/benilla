//! The glue stage's build: the select and create character and the select pet, assembled once a
//! frame in a schedule of their own between `PreUpdate` and `Update`, so the frame's booth stands
//! what they built.
//!
//! A select waits there for its whole stage, as the reference's does. Its build `0x472950` ends on
//! `IsReady(1, 1)` over the frame's scene model (`0x473083`, `0x710450`), which waits for each
//! model's file (`0x710520`) and loads every texture it binds (`0x44acf0` with load set, which
//! waits in `0x44ad50`), the character and pet attached to the scene included (`0x7104e8`), so
//! the select's own frame holds until the stage is whole and the next frame drawn shows it. Here
//! the reads stay on the asset server's threads and the frame waits on them, landing each file as
//! it finishes and running the steps whose next files it names: the display models, their forms
//! and the assembly. A create look builds on the asset server's own pace.

use std::time::{Duration, Instant};

use benilla_assets::materials::WowModelMaterial;
use benilla_assets::{M2Model, WmoModel};
use bevy::app::MainScheduleOrder;
use bevy::asset::{LoadState, RecursiveDependencyLoadState};
use bevy::ecs::schedule::{ExecutorKind, ScheduleLabel};
use bevy::prelude::*;

use super::preview::{build_glue_pet, build_glue_preview};
use crate::portrait::{
    GlueLook, GluePetBake, GluePreview, GluePreviewBake, GlueSceneFile, PreviewBillboard,
    PreviewPart, PreviewRider,
};

/// The glue stage's schedule, run each frame after `PreUpdate`'s asset landing.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct GlueStage;

/// How long a select may hold its frame for its stage.
///
/// Deviation: the wait gives up after this, with a warning, and what is left lands on the asset
/// server's pace, where the reference waits for ever, because a file that never lands would
/// otherwise freeze the screen.
const GLUE_STAGE_WAIT: Duration = Duration::from_secs(2);

/// A test's override of [`GLUE_STAGE_WAIT`].
#[derive(Resource)]
struct GlueStageWaitCap(Duration);

/// The wait's poll: the asset server's threads read and decode while the frame sleeps.
const GLUE_STAGE_POLL: Duration = Duration::from_millis(1);

/// Add [`GlueStage`] to the frame, after `PreUpdate`.
pub(in crate::entities) fn plugin(app: &mut App) {
    if app.get_schedule(GlueStage).is_some() {
        return;
    }
    let mut schedule = Schedule::new(GlueStage);
    // One exclusive system: nothing runs beside it, so no executor threads are needed.
    schedule.set_executor_kind(ExecutorKind::SingleThreaded);
    schedule.add_systems(build_glue_stage);
    app.add_schedule(schedule);
    if let Some(mut order) = app.world_mut().get_resource_mut::<MainScheduleOrder>() {
        order.insert_after(PreUpdate, GlueStage);
    }
}

/// The stage a select waited for: its look and its scene file.
type StageKey = (Option<GlueLook>, Option<GlueSceneFile>);

/// Build the stage's character and pet; on a select's first frame, until its stage is whole.
fn build_glue_stage(world: &mut World, mut waited: Local<Option<StageKey>>) {
    build(world);
    let Some(preview) = world.get_resource::<GluePreview>() else {
        return;
    };
    let key = (preview.look, preview.scene.clone());
    if !matches!(key.0, Some(GlueLook::Select(_))) || waited.as_ref() == Some(&key) {
        return;
    }
    *waited = Some(key);
    let scene = preview.scene.as_ref().map(|s| {
        world
            .resource::<AssetServer>()
            .load::<M2Model>(s.url().to_string())
    });
    let cap = world
        .get_resource::<GlueStageWaitCap>()
        .map_or(GLUE_STAGE_WAIT, |c| c.0);
    let start = Instant::now();
    let mut passes = 0u32;
    let whole = loop {
        if stage_whole(world, scene.as_ref()) {
            break true;
        }
        if start.elapsed() >= cap {
            break false;
        }
        std::thread::sleep(GLUE_STAGE_POLL);
        step(world);
        passes += 1;
    };
    if !whole {
        warn!(
            "glue stage: the select's stage was not whole after {:.1}s; the rest lands as it loads",
            cap.as_secs_f32()
        );
    }
    if benilla_assets::trace::enabled_for("gsel") {
        benilla_assets::trace::line(
            "gsel",
            &format!(
                "wait {:.1}ms passes={passes} whole={whole}",
                start.elapsed().as_secs_f64() * 1e3
            ),
        );
    }
}

/// The assembly of the character and the pet, each once per look. The pet has its own latch, so
/// outside a select's wait a slow pet model never holds the character back.
fn build(world: &mut World) {
    run(world, build_glue_preview);
    run(world, build_glue_pet);
}

/// One pass of the wait: land what the asset server finished, build the forms and display parts
/// it completes, and assemble, which names the items' files once the body stands.
fn step(world: &mut World) {
    bevy::asset::handle_internal_asset_events(world);
    world.try_resource_scope(
        |world, mut forms: Mut<benilla_world::model_forms::ModelForms>| {
            world.try_resource_scope(|world, mut meshes: Mut<Assets<Mesh>>| {
                if let (Some(m2s), Some(wmos)) = (
                    world.get_resource::<Assets<M2Model>>(),
                    world.get_resource::<Assets<WmoModel>>(),
                ) {
                    forms.furnish_now(m2s, wmos, &mut meshes);
                }
            });
        },
    );
    run(world, crate::entities::update_display_models);
    build(world);
}

fn run<M>(world: &mut World, system: impl IntoSystem<(), (), M> + 'static) {
    if let Err(e) = world.run_system_cached(system) {
        debug!("glue stage: a build step did not run: {e}");
    }
}

/// Whether the stage is whole: the look and the pet assembled, the scene's file and everything it
/// names landed, and every texture the assembly binds landed. A file that failed counts as
/// landed, as nothing more will come of it.
fn stage_whole(world: &World, scene: Option<&Handle<M2Model>>) -> bool {
    let preview = world.resource::<GluePreview>();
    let bake = world.resource::<GluePreviewBake>();
    let pet = world.resource::<GluePetBake>();
    let pet_want = preview
        .look
        .and_then(|l| l.pet())
        .map_or(0, |p| p.display_id);
    if bake.look != preview.look || pet.display_id != pet_want {
        return false;
    }
    let server = world.resource::<AssetServer>();
    if scene.is_some_and(|h| !landed_with_deps(server, h.id())) {
        return false;
    }
    let images = world.resource::<Assets<Image>>();
    let materials = world.resource::<Assets<WowModelMaterial>>();
    stage_textures(bake, pet, materials).all(|id| texture_landed(server, images, id))
}

/// Every texture the character's and the pet's assemblies bind: their batches' materials, each
/// blend twin's, and their emitters'.
fn stage_textures<'a>(
    bake: &'a GluePreviewBake,
    pet: &'a GluePetBake,
    materials: &'a Assets<WowModelMaterial>,
) -> impl Iterator<Item = AssetId<Image>> + 'a {
    let part = |p: &'a PreviewPart| {
        [
            Some(&p.material),
            p.twins.blend.as_ref(),
            p.twins.zfill.as_ref(),
        ]
    };
    let rider = |r: &'a PreviewRider| {
        [
            Some(&r.material),
            r.twins.blend.as_ref(),
            r.twins.zfill.as_ref(),
        ]
    };
    let card = |b: &'a PreviewBillboard| {
        [
            Some(&b.material),
            b.twins.blend.as_ref(),
            b.twins.zfill.as_ref(),
        ]
    };
    let batches = bake
        .parts
        .iter()
        .chain(&pet.parts)
        .flat_map(part)
        .chain(bake.riders.iter().flat_map(rider))
        .chain(bake.billboards.iter().chain(&pet.billboards).flat_map(card))
        .flatten()
        .filter_map(|h| materials.get(h))
        .flat_map(|m| {
            [
                m.base.base_color_texture.as_ref(),
                m.extension.stage1_texture.as_ref(),
            ]
        })
        .flatten()
        .map(Handle::id);
    let emitters = bake
        .effects
        .iter()
        .flat_map(|e| &e.emitters)
        .chain(&pet.emitters)
        .filter_map(|e| e.texture.as_ref())
        .map(Handle::id);
    batches.chain(emitters)
}

/// A texture is landed once it is in the store, or once the server says it never will be: failed,
/// or no load of the server's (a handle `Assets::add` made is in the store from the start).
fn texture_landed(server: &AssetServer, images: &Assets<Image>, id: AssetId<Image>) -> bool {
    images.contains(id) || !matches!(server.get_load_state(id), Some(LoadState::Loading))
}

/// A model file is landed once it and everything it names loaded, or any of them failed.
fn landed_with_deps(server: &AssetServer, id: AssetId<M2Model>) -> bool {
    matches!(server.get_load_state(id), Some(LoadState::Failed(_)))
        || matches!(
            server.get_recursive_dependency_load_state(id),
            Some(RecursiveDependencyLoadState::Loaded | RecursiveDependencyLoadState::Failed(_))
        )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::portrait::{PreviewEffects, SelectLook};
    use bevy::asset::io::memory::{Dir, MemoryAssetReader};
    use bevy::asset::io::{AssetSourceBuilder, Reader};
    use bevy::asset::{AssetLoader, LoadContext};

    /// How long the test loader takes over a texture: far longer than a frame.
    const SLOW_READ: Duration = Duration::from_millis(150);

    /// A slow disk: every `.slow` read takes [`SLOW_READ`] and yields a blank image.
    #[derive(Default, TypePath)]
    struct SlowImageLoader;

    impl AssetLoader for SlowImageLoader {
        type Asset = Image;
        type Settings = ();
        type Error = std::io::Error;

        async fn load(
            &self,
            reader: &mut dyn Reader,
            _settings: &(),
            _ctx: &mut LoadContext<'_>,
        ) -> Result<Image, Self::Error> {
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).await?;
            std::thread::sleep(SLOW_READ);
            Ok(Image::default())
        }

        fn extensions(&self) -> &[&str] {
            &["slow"]
        }
    }

    /// A hung disk: every `.hung` read stays loading for ever.
    #[derive(Default, TypePath)]
    struct HungImageLoader;

    impl AssetLoader for HungImageLoader {
        type Asset = Image;
        type Settings = ();
        type Error = std::io::Error;

        async fn load(
            &self,
            _reader: &mut dyn Reader,
            _settings: &(),
            _ctx: &mut LoadContext<'_>,
        ) -> Result<Image, Self::Error> {
            std::future::pending::<()>().await;
            unreachable!("a pending future never resolves")
        }

        fn extensions(&self) -> &[&str] {
            &["hung"]
        }
    }

    fn select(guid: u64) -> GlueLook {
        GlueLook::Select(SelectLook::from(
            &crate::glue_session::roster::test_character(guid, "Rec"),
        ))
    }

    /// The stage alone, over a memory source with the slow and hung loaders.
    fn stage_app() -> App {
        let dir = Dir::default();
        for file in ["a.slow", "b.slow", "c.hung"] {
            dir.insert_asset(Path::new(file), vec![0u8; 4]);
        }
        let mut app = App::new();
        app.register_asset_source(
            "mem",
            AssetSourceBuilder::new(move || Box::new(MemoryAssetReader { root: dir.clone() })),
        );
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
        app.init_asset::<Image>()
            .init_asset::<Mesh>()
            .init_asset::<WowModelMaterial>()
            .init_asset::<M2Model>()
            .init_asset::<WmoModel>()
            .register_asset_loader(SlowImageLoader)
            .register_asset_loader(HungImageLoader)
            .init_resource::<benilla_world::model_forms::ModelForms>()
            .init_resource::<GluePreview>()
            .init_resource::<GluePreviewBake>()
            .init_resource::<GluePetBake>();
        plugin(&mut app);
        app
    }

    /// `look` stands assembled, wearing one emitter whose texture is `file`, just asked for.
    fn stand(app: &mut App, look: GlueLook, file: &str) -> Handle<Image> {
        let texture: Handle<Image> = app
            .world()
            .resource::<AssetServer>()
            .load(format!("mem://{file}"));
        let emitter = benilla_assets::ModelEmitter {
            def: benilla_world::testing::plain_particle_def(),
            texture: Some(texture.clone()),
            bone_pivot: [0.0; 3],
            billboard: None,
            recursion: None,
            geometry: None,
            owner_reach: 0.0,
            water_bound: (Vec3::ZERO, 0.0),
            idle_seq: 0,
        };
        app.world_mut().resource_mut::<GluePreview>().look = Some(look);
        let mut bake = app.world_mut().resource_mut::<GluePreviewBake>();
        let revision = bake.revision + 1;
        *bake = GluePreviewBake {
            look: Some(look),
            effects: vec![PreviewEffects {
                bone: 0,
                offset: Vec3::ZERO,
                emitters: vec![emitter],
                rig: None,
            }],
            revision,
            ..default()
        };
        texture
    }

    fn landed(app: &App, texture: &Handle<Image>) -> bool {
        app.world().resource::<Assets<Image>>().contains(texture)
    }

    /// A select's frame does not go on while its stage is still being read (`0x473083` waits
    /// through `0x710450`): after one update its texture is in `Assets`, where a create look's
    /// frame, which does not wait, has none.
    #[test]
    fn a_select_frame_waits_for_its_stage() {
        let mut app = stage_app();
        let create = GlueLook::Create(crate::portrait::CreateLook {
            race: 1,
            sex: 0,
            class: 1,
            skin: 0,
            face: 0,
            hair_style: 0,
            hair_color: 0,
            facial_hair: 0,
        });
        let texture = stand(&mut app, create, "a.slow");
        app.update();
        assert!(
            !landed(&app, &texture),
            "the read takes {SLOW_READ:?}, so a frame that does not wait has no texture"
        );

        let mut app = stage_app();
        let texture = stand(&mut app, select(1), "a.slow");
        app.update();
        assert!(
            landed(&app, &texture),
            "the select's frame waited for its stage"
        );
    }

    /// The wait is the select's: the frames after it stand what lands, on the server's pace.
    #[test]
    fn a_select_waits_in_its_own_frame_only() {
        let mut app = stage_app();
        stand(&mut app, select(1), "a.slow");
        app.update();
        // The same look's later texture is the server's to land, so this frame does not hold.
        let later = stand(&mut app, select(1), "b.slow");
        app.update();
        assert!(
            !landed(&app, &later),
            "a frame after the select's does not wait"
        );
    }

    /// A read that never lands holds the select's frame only to the cap.
    #[test]
    fn a_hung_read_holds_the_frame_only_to_the_cap() {
        let mut app = stage_app();
        app.insert_resource(GlueStageWaitCap(Duration::from_millis(50)));
        let texture = stand(&mut app, select(1), "c.hung");
        let t0 = Instant::now();
        app.update();
        assert!(!landed(&app, &texture));
        assert!(
            t0.elapsed() < Duration::from_secs(1),
            "the frame went on at the cap, not after {:?}",
            t0.elapsed()
        );
    }
}
