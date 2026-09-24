//! **benilla-world** — the world renderer, with no game attached.
//!
//! Decision 1160's crate. The line it draws: *this crate can put something into a world and answer
//! questions about that world; it knows nothing about a session, a wire, an avatar or a UI.* What
//! enforces the line is not this doc — it is the dependency edge. `benilla-world` does not depend
//! on `benilla-app`, so a name that crosses back is a compile error rather than a lint, and
//! `benilla-worldview` links only this crate, so an engine that quietly needs the game stops
//! booting rather than quietly working.
//!
//! Above it: `benilla-app` (the game — session, wire, units, UI) and the two binaries.
//! Below it: `benilla-assets` (the asset foundation both sides share) and `benilla-formats`.
//!
//! **One instrument travels with the engine and three do not**, and the rule that sorts them is
//! whether `WorldPlugins` registers it. `art_scope` does — within-map art residency is engine
//! behaviour, not a readout — so it is here. The debug panel, the perf journal and the pipeline
//! warmer are registered by the game, and every one of them reads the game (units, targets, names,
//! the UI): they are instruments *above* the stack, and the compiler said so the moment the move
//! was attempted with them inside. That is the whole value of the crate edge over a naming
//! convention — the earlier module-name-based wall had them excluded by an assumption, and the
//! assumption was wrong.

/// Re-exported so a shim binary needs no `bevy` dependency of its own — the same courtesy
/// `benilla-app` extends to the `benilla` launcher.
pub use bevy::app::AppExit;

/// **Fixtures for dependents' tests.**
///
/// `#[cfg(test)]` code is not compiled for a downstream crate, so a fixture the game's tests share
/// with the engine's has to be ordinary public code. Three of `benilla-app`'s tests build a
/// particle emitter def — thirty fields of authored defaults — and duplicating that literal three
/// times is how two of the copies quietly drift from the type.
pub mod testing {
    use benilla_formats::{
        CellRamp, EmitParams, EmitTiming, OverLife, ParamsNow, ParticleBlend, ParticleEmitterDef,
        ParticleShape,
    };

    /// The sampled-parameter side of [`particle_def`], constant-baked so a sim test is
    /// deterministic.
    pub fn particle_params_now() -> ParamsNow {
        ParamsNow {
            emission_speed: 1.0,
            speed_variation: 0.0,
            vertical_range: 0.5,
            horizontal_range: std::f32::consts::PI,
            gravity: 0.0,
            lifespan: 1.0,
            area_length: 2.0,
            area_width: 4.0,
            z_source: 0.0,
        }
    }

    /// A minimal emitter def for kernel tests — only the fields the emit kernel reads matter.
    pub fn particle_def(shape: ParticleShape) -> ParticleEmitterDef {
        ParticleEmitterDef {
            flags: 0,
            position: [1.0, 2.0, 3.0],
            bone: 0,
            shape,
            blend: ParticleBlend::Add,
            lit: false,
            texture: None,
            tile_rows: 1,
            tile_cols: 1,
            head_tail: 0,
            timing: EmitTiming::constant(10.0),
            params: EmitParams::constant(particle_params_now()),
            drag: 0.0,
            tail_time: 0.0,
            spline: None,
            geometry_model: None,
            recursion_model: None,
            angular_velocity_min: [0.0; 3],
            angular_velocity_max: [0.0; 3],
            inherit_scale: 0.0,
            follow_speed1: 0.0,
            follow_scale1: 0.0,
            follow_speed2: 0.0,
            follow_scale2: 0.0,
            twinkle_speed: 0.0,
            twinkle_percent: 1.0,
            twinkle_min: 0.0,
            twinkle_max: 0.0,
            spin: 0.0,
            over_life: OverLife {
                mid: 0.5,
                color: [[1.0; 4]; 3],
                scale: [1.0; 3],
                head_cells: [CellRamp::new(0, 0); 2],
                tail_cells: [CellRamp::new(0, 0); 2],
                repeat: [1.0; 2],
            },
        }
    }

    /// The plain planar def most emitter tests want.
    pub fn plain_particle_def() -> ParticleEmitterDef {
        particle_def(ParticleShape::Plane)
    }

    /// A bind-pose [`crate::rig_anim::RigPose`] for tests that resolve consumer anchors
    /// (anchors spawn on first demand through `RigPose::anchor_for`, so a test
    /// wearer/mount/caster needs a pose, not a hand-built joint map): one root-parented joint per
    /// entry, seated at its local translation.
    pub fn test_rig_pose(
        root: bevy::ecs::entity::Entity,
        joints: &[bevy::math::Vec3],
    ) -> crate::rig_anim::RigPose {
        let skeleton = benilla_assets::ModelSkeleton {
            joints: joints
                .iter()
                .map(|&t| benilla_assets::ModelJoint {
                    parent: -1,
                    local_translation: t,
                    billboard: None,
                    parent_arm: None,
                })
                .collect(),
            spine_bone: None,
            head_bone: None,
        };
        crate::rig_anim::RigPose::new(root, &skeleton)
    }
}

pub mod art_scope;
pub mod assets;
pub mod bgwin;
pub mod billboard;
pub mod boot;
pub mod build_id;
pub mod clouds;
pub mod clutter;
pub mod collision;
pub mod decal;
pub mod dev_state;
pub mod doodad_anim;
pub mod entity_shade;
pub mod exterior_cull;
pub mod ffx_glow;
pub mod final_pass;
pub mod frame_pace;
pub mod ground_fx;
pub mod instance_tint;
pub mod interact;
pub mod interior;
pub mod lighting;
pub mod liquid;
pub mod log_ring;

/// macOS `Cmd+Q`, re-pointed at the window close so the gesture goes through an exit the client
/// can actually observe.
pub mod mac_quit;
pub mod map_proj;
pub mod mat_anim_table;
pub mod mesh_tag;
pub mod model_fade;
pub mod model_forms;
pub mod model_render;
pub mod modkeys;
pub mod particles;
pub mod ribbons;
pub mod ride_frame;
pub mod rig_anim;
pub mod rig_palette;
pub mod rig_rider;
pub mod schedule;
mod shaders;
pub mod sky;
pub mod sky_order;
pub mod skybox;
pub mod static_gx;
pub mod static_merge;
pub mod straddle;
pub mod sun;
pub mod surface;
pub mod terrain_stream;
pub mod thread_qos;
pub mod view;
pub mod vis_chain;
pub mod water_fx;
pub mod wdl;
pub mod weather;
pub mod wmo_portal;
pub mod world_census;
pub mod world_map;
pub mod world_plugins;
pub mod world_point;
pub mod world_unit;
pub mod worldview;
pub mod zfill;
