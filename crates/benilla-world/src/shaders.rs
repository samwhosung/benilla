//! The engine's WGSL, embedded in the binary; the game's shaders are embedded by `benilla-app`.
//! These calls must stay in a file directly under `src/`: `embedded_asset!` derives the served
//! path from `file!()` without normalizing it, so a call from a submodule would serve the shader
//! under that module's directory instead of `embedded://benilla_world/shaders/<name>.wgsl`.

use bevy::prelude::*;

/// Embed the engine's WGSL files under `embedded://benilla_world/shaders/`. The first member of
/// [`crate::world_plugins::WorldPlugins`]; it needs `DefaultPlugins`, which creates the asset
/// registry, added first.
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

    /// Reads each file on disk back through the embedded source a `ShaderRef` uses, so a shader
    /// that is unregistered, or registered from a submodule, fails here instead of drawing nothing.
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
        // One per `embedded_asset!` line in `plugin`.
        assert_eq!(found, 8, "the engine's shader set changed size");
    }
    /// The lines of `src` that take an implicit derivative (`textureSample`, `textureSampleBias`,
    /// `textureSampleCompare`, and the explicit builtins `dpdx`, `dpdy`, `fwidth` with their
    /// `Coarse` and `Fine` variants) below a function body's top level. WGSL allows them only in
    /// uniform control flow, and a branch on a fragment input is not. This reads text: it does not
    /// follow helper calls, a sample after a `return` inside a branch, or `/* */` comments.
    fn derivatives_in_nested_blocks(src: &str) -> Vec<usize> {
        const IMPLICIT: [&str; 12] = [
            "textureSample",
            "textureSampleBias",
            "textureSampleCompare",
            "dpdx",
            "dpdxCoarse",
            "dpdxFine",
            "dpdy",
            "dpdyCoarse",
            "dpdyFine",
            "fwidth",
            "fwidthCoarse",
            "fwidthFine",
        ];
        let mut depth = 0usize;
        let mut hits = Vec::new();
        for (n, line) in src.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            let mut word = String::new();
            for c in code.chars().chain(std::iter::once(' ')) {
                if c.is_alphanumeric() || c == '_' {
                    word.push(c);
                    continue;
                }
                if depth > 1 && IMPLICIT.contains(&word.as_str()) {
                    hits.push(n + 1);
                }
                word.clear();
                match c {
                    '{' => depth += 1,
                    '}' => depth = depth.saturating_sub(1),
                    _ => {}
                }
            }
        }
        hits
    }

    /// naga's check for this is compiled out of the fragment stage
    /// (`DISABLE_UNIFORMITY_REQ_FOR_FRAGMENT_STAGE`, naga 27 and 30), so these shaders validate
    /// natively and fail in a browser's compiler. This reads the text instead: no sample that
    /// takes implicit derivatives sits in a nested block, where a branch on a fragment input
    /// would make it non-uniform. Hoist the sample and `select` after.
    #[test]
    fn derivative_samples_stay_in_uniform_control_flow() {
        let bad = "fn fragment(in: V) -> vec4<f32> {\n    if (in.flag != 0u) {\n        let c = textureSampleBias(t, s, in.uv, 0.0);\n    }\n    let d = textureSample(t, s, in.uv);\n}\n";
        assert_eq!(
            derivatives_in_nested_blocks(bad),
            [3],
            "the scanner finds the nested sample"
        );

        let root = env!("CARGO_MANIFEST_DIR");
        for path in [
            "/src/shaders/static_gx.wgsl",
            "/../benilla-assets/src/shaders/terrain.wgsl",
        ] {
            let src = std::fs::read_to_string(format!("{root}{path}")).expect(path);
            assert_eq!(
                derivatives_in_nested_blocks(&src),
                Vec::<usize>::new(),
                "{path}"
            );
        }
    }
}
