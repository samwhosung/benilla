//! The game's five WGSL shaders, embedded in the binary. Registered from the root of `src/`: the
//! macro does not normalize paths, so a call from a submodule would bake its directory into the
//! served path.

use bevy::prelude::*;

/// Register the five shaders under `embedded://benilla_app/shaders/…`; added after `DefaultPlugins`
/// creates the registry it fills.
pub(crate) fn plugin(app: &mut App) {
    bevy::asset::embedded_asset!(app, "shaders/ui_quad.wgsl");
    bevy::asset::embedded_asset!(app, "shaders/ui_add.wgsl");
    bevy::asset::embedded_asset!(app, "shaders/ui_gamma.wgsl");
    bevy::asset::embedded_asset!(app, "shaders/ui_node_gamma.wgsl");
    bevy::asset::embedded_asset!(app, "shaders/ui_slice_gamma.wgsl");
}

#[cfg(test)]
mod tests {
    use bevy::asset::io::AssetSourceId;
    use bevy::prelude::*;

    /// Reads every file in the directory back from its embedded path.
    #[test]
    fn every_game_shader_answers_at_its_embedded_path() {
        let mut app = App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.add_plugins(super::plugin);
        let server = app.world().resource::<AssetServer>();
        let source = server.get_source(AssetSourceId::from("embedded")).unwrap();

        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/shaders");
        let mut found = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            if !name.ends_with(".wgsl") {
                continue;
            }
            found += 1;
            let path = format!("benilla_app/shaders/{name}");
            let read = bevy::tasks::block_on(async {
                source.reader().read(std::path::Path::new(&path)).await
            });
            assert!(
                read.is_ok(),
                "embedded://{path} does not resolve — {name} is on disk but not registered in \
                 `shaders::plugin`, or was registered from a file that is not directly under src/"
            );
        }
        assert_eq!(found, 5, "the game's shader set changed size");
    }

    /// Every `.wgsl` file under `dir` of the embedded source, as the asset server would serve it.
    fn embedded_shaders(
        source: &bevy::asset::io::AssetSource,
        dir: &std::path::Path,
        out: &mut Vec<(String, String)>,
    ) {
        use bevy::tasks::futures_lite::StreamExt;
        let reader = source.reader();
        let entries: Vec<std::path::PathBuf> = bevy::tasks::block_on(async {
            match reader.read_directory(dir).await {
                Ok(stream) => stream.collect().await,
                Err(_) => Vec::new(),
            }
        });
        for path in entries {
            if bevy::tasks::block_on(reader.is_directory(&path)).unwrap_or(false) {
                embedded_shaders(source, &path, out);
            } else if path.extension().is_some_and(|e| e == "wgsl") {
                let text = bevy::tasks::block_on(async {
                    let mut bytes = Vec::new();
                    let mut r = reader.read(&path).await.ok()?;
                    r.read_to_end(&mut bytes).await.ok()?;
                    String::from_utf8(bytes).ok()
                });
                if let Some(text) = text {
                    out.push((path.to_string_lossy().into_owned(), text));
                }
            }
        }
    }

    /// No gate draws a frame, so `ui_quad.wgsl` is composed here as the pipeline cache composes it:
    /// bevy's `ShaderCache` over every embedded shader library, with the defs the 2D mesh pipeline
    /// gives each of the two vertex layouts it draws (the batch mesh and the minimap's tile quad).
    /// The module must validate and translate for the backend this platform renders with.
    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "bevy's `ShaderCache` fixes the module loader's `PipelineCacheError` return"
    )]
    fn the_ui_quad_shader_composes_and_translates() {
        use bevy::shader::{Shader, ShaderCache, ShaderCacheSource, ShaderDefVal};
        let app = crate::game_plugins::schedule_tests::headless_client();
        let server = app.world().resource::<AssetServer>();
        let source = server.get_source(AssetSourceId::from("embedded")).unwrap();
        let mut files = Vec::new();
        embedded_shaders(source, std::path::Path::new(""), &mut files);

        let mut cache: ShaderCache<naga::Module, ()> = ShaderCache::new(
            bevy::render::render_resource::WgpuFeatures::empty(),
            bevy::render::render_resource::DownlevelFlags::all(),
            |_, source, _| match source {
                ShaderCacheSource::Naga(module) => Ok(module),
                _ => panic!("a WGSL shader composes to a naga module"),
            },
        );
        let mut ui_quad = None;
        for (i, (path, text)) in files.into_iter().enumerate() {
            let id = AssetId::<Shader>::Uuid {
                uuid: bevy::asset::uuid::Uuid::from_u128(i as u128 + 1),
            };
            if path == "benilla_app/shaders/ui_quad.wgsl" {
                ui_quad = Some(id);
            }
            cache.set_shader(id, Shader::from_wgsl(text, format!("embedded://{path}")));
        }
        let ui_quad = ui_quad.expect("ui_quad.wgsl is embedded");

        let storage =
            bevy::render::settings::WgpuLimits::default().max_storage_buffers_per_shader_stage;
        for layout in [
            ["VERTEX_POSITIONS", "VERTEX_UVS", "VERTEX_COLORS"],
            ["VERTEX_POSITIONS", "VERTEX_NORMALS", "VERTEX_UVS"],
        ] {
            let mut defs: Vec<ShaderDefVal> = layout.iter().map(|&d| d.into()).collect();
            defs.push(ShaderDefVal::UInt("MATERIAL_BIND_GROUP".into(), 2));
            defs.push(ShaderDefVal::UInt(
                "AVAILABLE_STORAGE_BUFFER_BINDINGS".into(),
                storage,
            ));
            let module = cache
                .get(&(), 0, ui_quad, &defs)
                .unwrap_or_else(|e| panic!("{layout:?}: ui_quad.wgsl does not compose: {e:?}"));
            let info = naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{layout:?}: ui_quad.wgsl does not validate: {e:?}"));
            #[cfg(target_os = "macos")]
            naga::back::msl::write_string(
                &module,
                &info,
                // wgpu's Metal backend writes at the device's MSL 2.x; 1.0, the default, lacks
                // the instance index every 2D mesh shader reads.
                &naga::back::msl::Options {
                    lang_version: (2, 4),
                    ..Default::default()
                },
                &naga::back::msl::PipelineOptions::default(),
            )
            .unwrap_or_else(|e| {
                panic!("{layout:?}: ui_quad.wgsl does not translate to MSL: {e:?}")
            });
            #[cfg(not(target_os = "macos"))]
            naga::back::spv::write_vec(&module, &info, &naga::back::spv::Options::default(), None)
                .unwrap_or_else(|e| {
                    panic!("{layout:?}: ui_quad.wgsl does not translate to SPIR-V: {e:?}")
                });
        }
    }
}
