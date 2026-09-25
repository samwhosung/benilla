//! **The game's WGSL, compiled into the binary**.
//!
//! The five shaders under `src/shaders/` draw the *game's* own surface — the UI quad lane, the
//! glue screen's additive pass, and the three gamma passes. 1171 moved them out of the engine's
//! asset root and into this crate; 1175 stops serving them from a directory at all.
//!
//! They used to reach the asset server through a `game://` source registered with
//! `concat!(env!("CARGO_MANIFEST_DIR"), "/assets")` — the build machine's source tree, baked into
//! the binary. `embedded_asset!` keeps 1171's line (it is per-crate by construction) and drops the
//! path: the interface cannot go missing because there is nothing to miss.
//!
//! The registration lives here, at the root of `src/`, for the path-arithmetic reason spelled out
//! in `benilla_world::shaders` — the macro does not normalize, so a call from a submodule would
//! bake that submodule's directory into the served path.

use bevy::prelude::*;

/// Compile the game's five WGSL files in and register them under
/// `embedded://benilla_app/shaders/…`. Runs before anything can ask for one — added with the
/// client's other plugins, after `DefaultPlugins` has created the registry it fills.
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

    /// The game's twin of `benilla_world::shaders`' test — see it for why the path is read back
    /// rather than reasoned about, and why the loop is driven off the directory listing.
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
