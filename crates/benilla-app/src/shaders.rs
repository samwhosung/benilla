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
}
