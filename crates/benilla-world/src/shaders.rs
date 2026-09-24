//! **The engine's WGSL, compiled into the binary**.
//!
//! The seven shaders under `src/shaders/` are the *engine's* — 1171's line, unchanged: the game's
//! five live in `benilla-app`, and `embedded_asset!` is per-crate by construction, so the seam
//! costs nothing to keep.
//!
//! Until 1175 these were served from a file asset root that `boot.rs` baked as
//! `concat!(env!("CARGO_MANIFEST_DIR"), "/assets")` — an absolute path into the *build* machine's
//! source tree. On any other machine it resolves to nothing and the world renders with no shaders
//! at all: the "silently-no-shaders trap" `capture/mod.rs`'s header already named. Embedding is
//! what makes the binary answer for its own content; `benilla-assets` has done it for its four
//! since it had them, and this is the same move for the other twelve.
//!
//! **Why these calls live in a file directly under `src/`.** `embedded_asset!` derives the served
//! path by stripping everything up to and including the crate's `src/` from `file!()`, then
//! re-prefixing the crate name — and it does **not** normalize the result. Called from
//! `src/sun/materials.rs` the shader would land at `embedded://benilla_world/sun/…`, and reaching
//! back out with `"../shaders/x.wgsl"` would bake a literal `..` component into the virtual
//! directory. One registration point at the root of `src/` is what keeps every path
//! `embedded://benilla_world/shaders/<name>.wgsl`.

use bevy::prelude::*;

/// Compile the engine's seven WGSL files in and register them under
/// `embedded://benilla_world/shaders/…`.
///
/// Added as the first member of [`crate::world_plugins::WorldPlugins`] rather than exposed as a
/// free function a composition root must remember to call: a shader that fails to register is
/// invisible until something tries to draw with it, which is the exact failure mode this record
/// closes. `Plugin::build` runs at `add_plugins` time, and both binaries add `DefaultPlugins`
/// (which creates the registry these calls fill) before `WorldPlugins`.
pub(crate) fn plugin(app: &mut App) {
    bevy::asset::embedded_asset!(app, "shaders/sky_vertex.wgsl");
    bevy::asset::embedded_asset!(app, "shaders/sky.wgsl");
    bevy::asset::embedded_asset!(app, "shaders/star.wgsl");
    bevy::asset::embedded_asset!(app, "shaders/cloud.wgsl");
    bevy::asset::embedded_asset!(app, "shaders/celestial.wgsl");
    bevy::asset::embedded_asset!(app, "shaders/ffx_glow.wgsl");
    bevy::asset::embedded_asset!(app, "shaders/wow_effect.wgsl");
    bevy::asset::embedded_asset!(app, "shaders/static_gx.wgsl");
}

#[cfg(test)]
mod tests {
    use bevy::asset::io::AssetSourceId;
    use bevy::prelude::*;

    /// Every `.wgsl` under `src/shaders/` answers at `embedded://benilla_world/shaders/<name>`.
    ///
    /// `embedded_asset!` derives that path from `file!()` and does **not** normalize it, so it is
    /// not something to reason about — this reads the bytes back through the same source a
    /// material's `ShaderRef` goes through. Driving the loop off the *directory listing* is the
    /// other half: a shader added without a registration line, or registered from a submodule
    /// (which silently prefixes that module's directory), fails here instead of drawing nothing.
    #[test]
    fn every_engine_shader_answers_at_its_embedded_path() {
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
            let path = format!("benilla_world/shaders/{name}");
            let read = bevy::tasks::block_on(async {
                source.reader().read(std::path::Path::new(&path)).await
            });
            assert!(
                read.is_ok(),
                "embedded://{path} does not resolve — {name} is on disk but not registered in \
                 `shaders::plugin`, or was registered from a file that is not directly under src/"
            );
        }
        // 6 since decision 1264 retired `skybox.wgsl` (a WMO skybox draws on the shared
        // model lane, whose forced-far-depth branch lives in `benilla_assets`'
        // `wow_model.wgsl`); 7 since 1429 added `static_gx.wgsl` (the B1 retained pass);
        // 8 since 2016 added `sky_vertex.wgsl` (the sky's shared far-depth vertex stage).
        assert_eq!(found, 8, "the engine's shader set changed size");
    }
}
