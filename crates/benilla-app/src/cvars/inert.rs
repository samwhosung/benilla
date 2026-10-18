//! The reference's script-visible CVars that nothing here reads. Lua's lookup (`0x63dec0`) finds a
//! record only with flag bit 31, which `CVar::Register` (`0x63db90`) sets for a `script_hidden`
//! argument of 0, and `GetCVar`, `SetCVar` and `GetCVarDefault` raise `Couldn't find CVar named`
//! on a miss (`0x488be7`, `0x488c53`, `0x488d37`). So every such record the reference registers is
//! registered here with its registered default, or an addon that sets one stops with an error.
//!
//! A row is inert: no observer reads it, `GetCVar` answers what was written, and it persists like
//! any row, as a `[cvars]` entry in `benilla-config/config.toml` only while off its default. The
//! flag bit1 rows are latched, as in the reference (a write is kept until a restart that never
//! comes for the sound block, and `RestartGx` commits the `gx*` pair). A row whose callback
//! refuses a write carries it as its check: the camera block's (`0x50b380`, `0x50b3c0`,
//! `0x50bfe0`, `0x50c000`, `0x50c040`), the error filter's (`0x4030f0`), the target range's
//! (`0x490720`) and the world's (`0x688b70`, `0x688bd0`, `0x688f00`, `0x688fb0`, `0x689000`,
//! `0x689050`, `0x689250`, `0x689450`, `0x6d8530`). The two LOD distances' (`0x688ab0`,
//! `0x6891f0`) never refuse but print a line out of range, and run too. The trailing address is
//! the register site in `reference/cvars.tsv`.

use super::table::{same, Registered};

pub(super) const INERT: &[Registered] = &[
    // ── The camera block ─────────────────────────────────────────────────────────────────────
    same("cameraCustomViewSmoothing", "0"), // 0x50bad0
    same("cameraDistanceMoveSpeed", "8.33").checked(benilla_ui::script::check_camera_decay), // 0x50b7f3
    same("cameraDistanceSmoothSpeed", "8.33").checked(benilla_ui::script::check_camera_decay), // 0x50b8a2
    same("cameraDive", "1"), // 0x50be03
    same("cameraFoVSmoothSpeed", "0.5").checked(benilla_ui::script::check_camera_rate), // 0x50b87f
    same("cameraHeightIgnoreStandState", "0"), // 0x50bd83
    same("cameraHeightSmoothSpeed", "1.2").checked(benilla_ui::script::check_camera_decay), // 0x50b8e8
    same("cameraPitchSmoothMax", "30.0").checked(benilla_ui::script::check_camera_pitch), // 0x50bf18
    same("cameraPitchSmoothMin", "0.0").checked(benilla_ui::script::check_camera_pitch), // 0x50bef5
    same("cameraSmoothAlwaysFearDelay", "0.0"),                                          // 0x50bb5e
    same("cameraSmoothAlwaysFearFactor", "1.0"),                                         // 0x50bb5e
    same("cameraSmoothAlwaysIdleDelay", "0.0"),                                          // 0x50bb5e
    same("cameraSmoothAlwaysIdleFactor", "1.0"),                                         // 0x50bb5e
    same("cameraSmoothAlwaysMoveDelay", "0.0"),                                          // 0x50bb5e
    same("cameraSmoothAlwaysMoveFactor", "1.0"),                                         // 0x50bb5e
    same("cameraSmoothAlwaysStopDelay", "0.0"),                                          // 0x50bb5e
    same("cameraSmoothAlwaysStopFactor", "1.0"),                                         // 0x50bb5e
    same("cameraSmoothAlwaysStrafeDelay", "0.0"),                                        // 0x50bb5e
    same("cameraSmoothAlwaysStrafeFactor", "1.0"),                                       // 0x50bb5e
    same("cameraSmoothAlwaysTrackDelay", "0.0"),                                         // 0x50bb5e
    same("cameraSmoothAlwaysTrackFactor", "1.0"),                                        // 0x50bb5e
    same("cameraSmoothAlwaysTurnDelay", "0.0"),                                          // 0x50bb5e
    same("cameraSmoothAlwaysTurnFactor", "1.0"),                                         // 0x50bb5e
    same("cameraSmoothNeverFearDelay", "0.0"),                                           // 0x50bb5e
    same("cameraSmoothNeverFearFactor", "0.0"),                                          // 0x50bb5e
    same("cameraSmoothNeverIdleDelay", "0.0"),                                           // 0x50bb5e
    same("cameraSmoothNeverIdleFactor", "0.0"),                                          // 0x50bb5e
    same("cameraSmoothNeverMoveDelay", "0.0"),                                           // 0x50bb5e
    same("cameraSmoothNeverMoveFactor", "0.0"),                                          // 0x50bb5e
    same("cameraSmoothNeverStopDelay", "0.0"),                                           // 0x50bb5e
    same("cameraSmoothNeverStopFactor", "0.0"),                                          // 0x50bb5e
    same("cameraSmoothNeverStrafeDelay", "0.0"),                                         // 0x50bb5e
    same("cameraSmoothNeverStrafeFactor", "0.0"),                                        // 0x50bb5e
    same("cameraSmoothNeverTrackDelay", "0.0"),                                          // 0x50bb5e
    same("cameraSmoothNeverTrackFactor", "0.0"),                                         // 0x50bb5e
    same("cameraSmoothNeverTurnDelay", "0.0"),                                           // 0x50bb5e
    same("cameraSmoothNeverTurnFactor", "0.0"),                                          // 0x50bb5e
    same("cameraSmoothPitch", "0"),                                                      // 0x50ba55
    same("cameraSmoothSmartFearDelay", "0.4"),                                           // 0x50bb5e
    same("cameraSmoothSmartFearFactor", "10.0"),                                         // 0x50bb5e
    same("cameraSmoothSmartIdleDelay", "0.0"),                                           // 0x50bb5e
    same("cameraSmoothSmartIdleFactor", "0.0"),                                          // 0x50bb5e
    same("cameraSmoothSmartMoveDelay", "0.0"),                                           // 0x50bb5e
    same("cameraSmoothSmartMoveFactor", "1.0"),                                          // 0x50bb5e
    same("cameraSmoothSmartStopDelay", "0.0"),                                           // 0x50bb5e
    same("cameraSmoothSmartStopFactor", "0.0"),                                          // 0x50bb5e
    same("cameraSmoothSmartStrafeDelay", "0.0"),                                         // 0x50bb5e
    same("cameraSmoothSmartStrafeFactor", "1.0"),                                        // 0x50bb5e
    same("cameraSmoothSmartTrackDelay", "0.4"),                                          // 0x50bb5e
    same("cameraSmoothSmartTrackFactor", "10.0"),                                        // 0x50bb5e
    same("cameraSmoothSmartTurnDelay", "0.0"),                                           // 0x50bb5e
    same("cameraSmoothSmartTurnFactor", "1.0"),                                          // 0x50bb5e
    same("cameraSmoothTimeMax", "2.0").checked(benilla_ui::script::check_camera_time),   // 0x50bfa4
    same("cameraSmoothTimeMin", "0.1").checked(benilla_ui::script::check_camera_time),   // 0x50bf81
    same("cameraSmoothViewDataAlwaysDistanceDelay", "0.0"),                              // 0x50bbfe
    same("cameraSmoothViewDataAlwaysDistanceFactor", "0.0"),                             // 0x50bbfe
    same("cameraSmoothViewDataAlwaysPitchDelay", "0.0"),                                 // 0x50bbfe
    same("cameraSmoothViewDataAlwaysPitchFactor", "0.0"),                                // 0x50bbfe
    same("cameraSmoothViewDataAlwaysYawDelay", "0.0"),                                   // 0x50bbfe
    same("cameraSmoothViewDataAlwaysYawFactor", "1.0"),                                  // 0x50bbfe
    same("cameraSmoothViewDataNeverDistanceDelay", "0.0"),                               // 0x50bbfe
    same("cameraSmoothViewDataNeverDistanceFactor", "0.0"),                              // 0x50bbfe
    same("cameraSmoothViewDataNeverPitchDelay", "0.0"),                                  // 0x50bbfe
    same("cameraSmoothViewDataNeverPitchFactor", "0.0"),                                 // 0x50bbfe
    same("cameraSmoothViewDataNeverYawDelay", "0.0"),                                    // 0x50bbfe
    same("cameraSmoothViewDataNeverYawFactor", "0.0"),                                   // 0x50bbfe
    same("cameraSmoothViewDataSmartDistanceDelay", "0.0"),                               // 0x50bbfe
    same("cameraSmoothViewDataSmartDistanceFactor", "0.0"),                              // 0x50bbfe
    same("cameraSmoothViewDataSmartPitchDelay", "0.0"),                                  // 0x50bbfe
    same("cameraSmoothViewDataSmartPitchFactor", "1.0"),                                 // 0x50bbfe
    same("cameraSmoothViewDataSmartYawDelay", "0.0"),                                    // 0x50bbfe
    same("cameraSmoothViewDataSmartYawFactor", "1.0"),                                   // 0x50bbfe
    same("cameraSmoothYaw", "1"),                                                        // 0x50ba71
    same("cameraSubmergeFinalPitch", "5.0").checked(benilla_ui::script::check_camera_pitch), // 0x50be8f
    same("cameraSubmergePitch", "18.0").checked(benilla_ui::script::check_camera_pitch), // 0x50be49
    same("cameraSurfaceFinalPitch", "5.0").checked(benilla_ui::script::check_camera_pitch), // 0x50be6c
    same("cameraSurfacePitch", "0.0").checked(benilla_ui::script::check_camera_pitch), // 0x50be26
    same("cameraTerrainTiltAlwaysFallAbsorb", "1.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltAlwaysFallDelay", "0.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltAlwaysFallFactor", "0.75"),                                 // 0x50bc9e
    same("cameraTerrainTiltAlwaysFearAbsorb", "1.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltAlwaysFearDelay", "0.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltAlwaysFearFactor", "1.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltAlwaysIdleAbsorb", "1.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltAlwaysIdleDelay", "0.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltAlwaysIdleFactor", "1.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltAlwaysJumpAbsorb", "0.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltAlwaysJumpDelay", "0.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltAlwaysJumpFactor", "-1.0"),                                 // 0x50bc9e
    same("cameraTerrainTiltAlwaysMoveAbsorb", "1.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltAlwaysMoveDelay", "0.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltAlwaysMoveFactor", "1.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltAlwaysStrafeAbsorb", "1.0"),                                // 0x50bc9e
    same("cameraTerrainTiltAlwaysStrafeDelay", "0.0"),                                 // 0x50bc9e
    same("cameraTerrainTiltAlwaysStrafeFactor", "1.0"),                                // 0x50bc9e
    same("cameraTerrainTiltAlwaysSwimAbsorb", "0.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltAlwaysSwimDelay", "0.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltAlwaysSwimFactor", "1.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltAlwaysTaxiAbsorb", "0.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltAlwaysTaxiDelay", "0.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltAlwaysTaxiFactor", "1.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltAlwaysTrackAbsorb", "1.0"),                                 // 0x50bc9e
    same("cameraTerrainTiltAlwaysTrackDelay", "0.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltAlwaysTrackFactor", "1.0"),                                 // 0x50bc9e
    same("cameraTerrainTiltAlwaysTurnAbsorb", "1.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltAlwaysTurnDelay", "0.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltAlwaysTurnFactor", "1.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltNeverFallAbsorb", "0.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltNeverFallDelay", "0.0"),                                    // 0x50bc9e
    same("cameraTerrainTiltNeverFallFactor", "-1.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltNeverFearAbsorb", "0.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltNeverFearDelay", "0.0"),                                    // 0x50bc9e
    same("cameraTerrainTiltNeverFearFactor", "-1.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltNeverIdleAbsorb", "0.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltNeverIdleDelay", "0.0"),                                    // 0x50bc9e
    same("cameraTerrainTiltNeverIdleFactor", "-1.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltNeverJumpAbsorb", "0.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltNeverJumpDelay", "0.0"),                                    // 0x50bc9e
    same("cameraTerrainTiltNeverJumpFactor", "-1.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltNeverMoveAbsorb", "0.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltNeverMoveDelay", "0.0"),                                    // 0x50bc9e
    same("cameraTerrainTiltNeverMoveFactor", "-1.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltNeverStrafeAbsorb", "0.0"),                                 // 0x50bc9e
    same("cameraTerrainTiltNeverStrafeDelay", "0.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltNeverStrafeFactor", "-1.0"),                                // 0x50bc9e
    same("cameraTerrainTiltNeverSwimAbsorb", "0.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltNeverSwimDelay", "0.0"),                                    // 0x50bc9e
    same("cameraTerrainTiltNeverSwimFactor", "-1.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltNeverTaxiAbsorb", "0.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltNeverTaxiDelay", "0.0"),                                    // 0x50bc9e
    same("cameraTerrainTiltNeverTaxiFactor", "-1.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltNeverTrackAbsorb", "0.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltNeverTrackDelay", "0.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltNeverTrackFactor", "-1.0"),                                 // 0x50bc9e
    same("cameraTerrainTiltNeverTurnAbsorb", "0.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltNeverTurnDelay", "0.0"),                                    // 0x50bc9e
    same("cameraTerrainTiltNeverTurnFactor", "-1.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltSmartFallAbsorb", "1.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltSmartFallDelay", "0.0"),                                    // 0x50bc9e
    same("cameraTerrainTiltSmartFallFactor", "0.75"),                                  // 0x50bc9e
    same("cameraTerrainTiltSmartFearAbsorb", "1.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltSmartFearDelay", "0.0"),                                    // 0x50bc9e
    same("cameraTerrainTiltSmartFearFactor", "1.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltSmartIdleAbsorb", "0.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltSmartIdleDelay", "0.0"),                                    // 0x50bc9e
    same("cameraTerrainTiltSmartIdleFactor", "-1.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltSmartJumpAbsorb", "0.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltSmartJumpDelay", "0.0"),                                    // 0x50bc9e
    same("cameraTerrainTiltSmartJumpFactor", "-1.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltSmartMoveAbsorb", "1.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltSmartMoveDelay", "0.0"),                                    // 0x50bc9e
    same("cameraTerrainTiltSmartMoveFactor", "1.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltSmartStrafeAbsorb", "1.0"),                                 // 0x50bc9e
    same("cameraTerrainTiltSmartStrafeDelay", "0.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltSmartStrafeFactor", "1.0"),                                 // 0x50bc9e
    same("cameraTerrainTiltSmartSwimAbsorb", "0.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltSmartSwimDelay", "0.0"),                                    // 0x50bc9e
    same("cameraTerrainTiltSmartSwimFactor", "1.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltSmartTaxiAbsorb", "0.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltSmartTaxiDelay", "0.0"),                                    // 0x50bc9e
    same("cameraTerrainTiltSmartTaxiFactor", "1.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltSmartTrackAbsorb", "1.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltSmartTrackDelay", "0.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltSmartTrackFactor", "1.0"),                                  // 0x50bc9e
    same("cameraTerrainTiltSmartTurnAbsorb", "1.0"),                                   // 0x50bc9e
    same("cameraTerrainTiltSmartTurnDelay", "0.0"),                                    // 0x50bc9e
    same("cameraTerrainTiltSmartTurnFactor", "1.0"),                                   // 0x50bc9e
    same("cameraViewBlendStyle", "1"),                                                 // 0x50b972
    same("cameraYawSmoothMax", "0.0").checked(benilla_ui::script::check_camera_yaw),   // 0x50bf5e
    same("cameraYawSmoothMin", "0.0").checked(benilla_ui::script::check_camera_yaw),   // 0x50bf3b
    same("camerasmooth", "1"),                                                         // 0x50ba39
    // ── The sound block: flag bit1, read once at sound init ───────────────────────────────────
    same("SoundDriver", "-1").latched(),              // 0x457206
    same("SoundInitFlags", "128").latched(),          // 0x4572ba
    same("SoundMaxHardwareChannels", "12").latched(), // 0x457260
    same("SoundMemoryCache", "4").latched(),          // 0x4572d8
    same("SoundMinHardwareChannels", "-1").latched(), // 0x457242
    same("SoundMixRate", "44100").latched(),          // 0x45727e
    same("SoundMixer", "-1").latched(),               // 0x457224
    same("SoundOutputSystem", "-1").latched(),        // 0x4571e8
    same("SoundRolloffFactor", "4").latched(),        // 0x4572f6
    same("SoundSoftwareChannels", "12").latched(),    // 0x45729c
    // ── Everything else ──────────────────────────────────────────────────────────────────────
    same("automoveturnspeednarrow", "800"), // 0x6039ee
    same("automoveturnspeedwide", "1200"),  // 0x6039d4
    same("bspcache", "1"),                  // 0x688868
    same("combatLogOn", "1"),               // 0x48fca4
    same("CombatModeMaxDistance", "30.0f"), // 0x5e24a9
    same("debugTaint", "0"),                // 0x48fc18
    same("disableOptionalSpeech", "0"),     // 0x40239d
    same("DistCull", "500").checked(benilla_ui::script::check_dist_cull), // 0x688570
    same("EnableGroupSpeech", "1"),         // 0x457859
    same("ErrorFilter", "all").checked(benilla_ui::script::check_error_filter), // 0x402d2a
    same("ErrorLevelMax", "3"),             // 0x402d07
    same("ErrorLevelMin", "1"),             // 0x402ce4
    same("Errors", "0"),                    // 0x402c9e
    same("ffxRectangle", "1"),              // 0x6cd007
    same("footstepBias", "0.125").checked(benilla_ui::script::check_footstep_bias), // 0x6888b4
    same("FootstepSounds", "1"),            // 0x623133
    same("fullAlpha", "0"),                 // 0x6884b2
    same("gxAspect", "1").latched(),        // 0x63a8af
    same("gxMultisampleQuality", "0.0").latched(), // 0x63a976
    same("gxOverride", ""),                 // 0x63a9e7
    same("horizonfarclip", "2112").checked(benilla_ui::script::check_horizon_farclip), // 0x68881c
    same("hwDetect", "1"),                  // 0x63a259
    same("Joystick", "0"),                  // 0x5142bc
    same("lodDist", "100.0").checked(benilla_ui::script::check_lod_dist), // 0x688524
    same("M2BatchDoodads", "1"),            // 0x40292f
    same("M2Faster", "1"),                  // 0x402952
    same("M2FasterDebug", "0"),             // 0x402975
    same("M2UseClipPlanes", "1"),           // 0x4028a3
    same("M2UseThreads", "1"),              // 0x4028c6
    same("M2UseZFill", "1"),                // 0x402880
    same("mapObjLightLOD", "0").checked(benilla_ui::script::check_map_obj_light_lod), // 0x6886ec
    same("mapObjOverbright", "1"),          // 0x6886c6
    same("mapShadows", "1"),                // 0x6884fe
    same("MapWaterSounds", "1"),            // 0x462ac9
    same("MaxLights", "4").checked(benilla_ui::script::check_max_lights), // 0x688596
    same("mouseInvertYaw", "0"),            // 0x50b72d
    same("movie", "1"),                     // 0x402bf5
    same("ObjectSelectionCircle", "1"),     // 0x6146ec
    same("occlusion", "1"),                 // 0x6888da
    same("particleDensity", "1.0").checked(benilla_ui::script::check_particle_density), // 0x688738
    same("PlayerAnim", "0"),                // 0x60333b
    same("PlayerFadeInRate", "4096"),       // 0x481b72
    same("PlayerFadeOutAlpha", "128"),      // 0x481bbf
    same("PlayerFadeOutRate", "4096"),      // 0x481b91
    same("shadowBias", "0.1").checked(benilla_ui::script::check_shadow_bias), // 0x6d817d
    same("shadowLOD", "1"),                 // 0x6d8157
    same("ShowErrors", "1"),                // 0x402cc1
    same("showfootprintparticles", "1"),    // 0x6039b5
    same("showfootprints", "1"),            // 0x688842
    same("showsmartrects", "0"),            // 0x509439
    same("SkyCloudLOD", "0"),               // 0x6d1d33
    same("TargetAnim", "0"),                // 0x60335e
    same("targetNearestDistance", "41.000000")
        .checked(benilla_ui::script::check_target_nearest_distance), // 0x48fda4
    same("targetNearestDistanceRadius", "10.000000")
        .checked(benilla_ui::script::check_target_nearest_distance), // 0x48fd60
    same("textureLodDist", "777.0").checked(benilla_ui::script::check_texture_lod_dist), // 0x6887f6
    same("timingModeOverride", "0"),        // 0x402eb4
    same("triangleStrips", "1"),            // 0x68888e
    same("unitDrawDist", "300.0").checked(benilla_ui::script::check_unit_draw_dist), // 0x68875e
    same("UnitNameRenderMode", "2"),        // 0x48fdc3
    same("violenceLevel", "2"),             // 0x6c5ae1
    same("waterLOD", "0").checked(benilla_ui::script::check_water_lod), // 0x688784
    same("widescreen", "1"),                // 0x63a74f
];
