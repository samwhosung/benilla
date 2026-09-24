//! Liquid (water) rendering: the animated lake/river/ocean surfaces the reference draws over MCLQ
//! geometry. The parse + flat mesh live in `benilla_formats::liquid` (built into each `ChunkMesh.liquid`
//! by the terrain loader); this is the Bevy render glue — one shared [`LiquidMaterial`] per
//! [`LiquidKind`], a `texture_2d_array` of its animated frames, and a 24 fps frame-index cycler.
//!
//! **This is the ADT path, and we render WMO liquid through it too — which is wrong.** The reference
//! has three liquid renderers, not one: ADT MCLQ (below), WMO MLIQ water (`0x6b62e0` category 0,
//! split again into exterior `0x6b6630` / interior `0x6b6420` on the group's `MOGP.flags & 0x48` —
//! one texture stage, no depth ramp, alpha from a per-vertex authored byte), and magma/slime. The
//! `wmo_liquid_arms` census sizes them: 30 exterior water groups (Stormwind's canals) against 134
//! interior (every dungeon pool, Blackfathom's included). Neither is what this file implements.
//!
//! Faithful ADT model — the combiner is an **asset**, extracted from `patch.MPQ` and read, not
//! inferred: `Shaders\Pixel\ocean0_s.bls` is `rgb = primary·colorTex.rgb + detailTex.rgb +
//! (secondary+0.25)·detailTex.a`, `alpha = colorTex.a`, with `0.25` its own scalar `PARAM`. The
//! formula this module has always carried is right verbatim; its **provenance was not** — the
//! citation here used to name an "apitrace WoW.17 program 159" and a `docs/knowledge/terrain.md`,
//! neither of which exists. The body
//! colour is **`primary · waterTint`**, where:
//! - **`waterTint`** is a **64-row byte-space ramp** between the zone's dedicated `Light.dbc` water
//!   rows, RAW (no ×0.711): IntBand rows 16/17 (river/lake) or 14/15 (ocean), shallow→deep, by the
//!   per-vertex depth `V` (river/lake `V = clamp(byte/42)`, VERIFIED `c81768`/`FUN_0068d790`; saturates
//!   ~5 yd so the channel middle reaches the deep/teal row). `FUN_0068a830` builds it as an exact
//!   integer accumulator — `row(i) = c0 + floor(i·(c1−c0)/64)`, `i = 0..63` — so it **never reaches
//!   the deep endpoint**, and it is sampled LINEAR/CLAMP at texel `V·64 − 0.5`. **The ocean's last
//!   row alone is then darkened** `floor(0.9·byte)` per channel (an HSV `V ×= 0.9` that reduces to
//!   exactly that) with its alpha forced opaque — and ~80 % of the world's ocean vertices sample
//!   that one row, so it is the open sea's actual colour (decision 2074). The swatch is rebuilt **per world frame**,
//!   not baked once at bootstrap: the dirty flags `[0xc8117c]`/`[0xc81b70]` clear at
//!   `0x680b90`/`0x680b97` and refill via `0x58acd0`, so colour and opacity track the zone and the
//!   clock. (The earlier "reflected sky × 0.711 via `FUN_0068c250`" model fingered the WRONG builder —
//!   a separate grey edge texture never bound on the water unit; and `byte/255` was the wrong LUT →
//!   river never went teal. The `FUN_0068a830` attribution this line used to carry was also wrong:
//!   `0x68a830` is a sky-band row fill.)
//! - **`primary`** is the lit vertex colour `clamp(ambient + N·L·sun)`.
//! - the animated `lake_a`/`ocean_h` frame is the **`detailTex`** (near-black RGB + ripple alpha): a
//!   faint flat lift + an achromatic shimmer on crests — NOT the body colour. Mipped + 16× aniso so the
//!   ripple averages out at distance (near-field samples mip 0, so near sparkle is the term itself).
//! - **opacity** = the SAME `V` indexes both colour and alpha (one swatch row → RGB + A): the same
//!   64-row ramp between
//!   the LightParams shallow/deep alphas — river 0.5→1.0, ocean 0.75→1.0. The river channel reaches
//!   α=1.0 (opaque, deep teal) by byte 42 ≈ 5 yd; the shore stays see-through (the pale edge band —
//!   the bottom shows). The linear ramp is right, and `0xc7fbc0`'s `1.6·(i/63)^8` — which this line
//!   used to flag as a competing reading — is **not on this path at all**: its only consumer is
//!   `0x68c250` (via `0x68c4a0`) building the separate texture `[0xc7fbb8]`, bound at `0x685257`
//!   inside a branch proven **dead** (`[0xc800ec]` is BSS whose only writer stores 0 and which is
//!   never a `0x58b2b0` out-param target, so both arms are controlled). It is the sky/overlay glare
//!   falloff, not a liquid opacity curve. The swatch's alpha column is the same endpoint lerp its RGB
//!   is (`0x68a830`).
//!
//! **Each kind reads its own depth LUT, and both are verified** — `FUN_0068c4c0` builds them side by
//! side and the two vert-fills read them the same way, `tc0 = (0.5, ramp[depthByte])`: river/lake
//! `c81768 = min(d/42, 1)` (`FUN_0068d790`, `fld` @`0x68d818`), ocean `c7fcd8 = min(d/255, 1)`
//! (`FUN_0068d690`, `fld` @`0x68d718`). Putting the river's `/42` on a river was the 2026-05-31 fix;
//! the ocean's `/255` was never the same mistake, and calling it a placeholder was ours, not the
//! binary's — `0x68d890`, the address that claim rested on, is the **magma** vert-fill (authored
//! per-vertex texcoords × 3/256), as `benilla_adt::LiquidVertex::texcoords` has always said.
//! The two divisors differ because the two authored depth bytes do: measured over every MCLQ block in
//! Azeroth + Kalimdor, the sea's byte ramps **1.72 byte/yd** against a river's **8.96**, so `/255`
//! saturates at ~148 yd of depth and `/42` at ~4.7 yd — within ~15 % of each other in yards per unit
//! of V — and 83.4 % of ocean vertices are pinned at byte 255 outright, so the open sea is authored
//! fully deep and the ramp is a shore band. Decision 2069;
//! `benilla-formats::examples::liquid_depth_census` is the instrument, `WOW_OCEAN_DEPTH_DIV` the A/B.
//! (Earlier cuts: ripple-as-colour → black; `×8` → "deep too early"; FLAT colour → "completely gone";
//! sky × 0.711 → wrong builder; `byte/255` on the RIVER → wrong LUT, no teal centre. Faithful =
//! rows 14–17 raw lerp + the /42 V on rivers, the /255 V on the sea.)
//!
//! Two-sided, alpha-blended, depth-write off (Bevy's transparent pass = the verified MCLQ water render
//! state).
//!
//! The frame-flip is the client's first render animation — a deliberate **one-off** (a frame-index
//! uniform off Bevy real `Time`), NOT a general animation system. Two clocks: animation =
//! wall-clock; day/night = server game-time.

//! ## Layout
//!
//! Three concerns, so three files behind this face:
//!
//! * [`query`] — **where the liquid is, and whether you are in it.** The world-space grid every
//!   surface publishes ([`WaterChunkInfo`]), the position queries the swim/sound/foam/lighting
//!   systems ask it ([`liquid_at`], [`water_surface_at`]), and the per-frame camera submersion
//!   verdict ([`Underwater`]). Nothing here renders.
//! * [`spatial`] — **which surfaces are near an XY**, without walking them all: the grid-hash
//!   [`WaterIndex`] the per-draw consumers (the water-plane interleave's three lanes) pre-filter
//!   through. The once-a-frame askers keep the plain walk.
//! * [`surface`] — **the Bevy render glue.** The per-kind animated materials, the two spawn paths
//!   (MCLQ and WMO MLIQ), the flat mesh build, and the 24 fps frame cycler.
//! * [`drift`] — **the underwater drift cloud**: the 4000-mote field the reference draws while the
//!   camera eye is inside a liquid (decision 1814). It is here rather than under `weather` because
//!   the reference keeps it that way too — the pool is CWorld's, not the weather manager's, and
//!   the two share no state and no code — and because [`Underwater`] is its whole trigger.
//!
//! The one cross-feed runs query → lighting: `detect_submersion` publishes WHICH liquid the eye is
//! in, and `lighting::update_time_lighting` selects the whole submerged atmosphere from it.

use bevy::pbr::MaterialPlugin;
use bevy::prelude::*;

use benilla_assets::materials::LiquidMaterial;
use benilla_assets::AssetSet;

mod drift;
mod query;
#[cfg(test)]
mod real_data;
mod spatial;
mod surface; // the against-real-client-files tests — they span both halves

// The submodules are private, so this list IS the subsystem's face: everything the rest of the
// client may name. Most of this list is now reached through `crate::world_point::WorldPoint`
// rather than directly (decision 1164).
//
// `LiquidSurface` used to be deliberately absent — "spawned, never named from outside; add it here
// the day something needs it". Decision 1652 is that day: the exterior-window cull counts liquid
// apart from the rest of the scene, because a few dozen surfaces summed into tens of thousands of
// terrain cells is a leg that could reach nothing at all and never show it.
pub use query::{
    camera_claim, describe_at, liquid_at, player_claim, surfaces_at, unit_claim, water_surface_at,
    EyeLiquid, FoamPatch, LiquidClaim, LiquidHit, LiquidSource, RoomPlacements, SubmergedEye,
    Underwater, WaterChunkInfo, WmoPool,
};
pub(crate) use spatial::{maintain_water_index, WaterIndex};
pub(crate) use surface::{
    spawn_liquids, spawn_wmo_liquids, LiquidAssets, LiquidSoundSource, LiquidSurface,
};

/// `WOW_FORCE_SUB=<frames>` — **hold the camera-eye verdict submerged for the first `<frames>`
/// frames, then release it.** The surfacing crossing on demand, with no server, no swim and no
/// water under the camera: everything that forks on the verdict (the atmosphere, the sky and
/// cloud domes' gates, the drift cloud, the FFX haze and warp) takes the wet→dry edge on a frame
/// this names, so a transition artefact can be photographed or logged deterministically inside
/// the capture harness (`WOW_CAPTURE=<scenario>`).
///
/// Built for B354 (decision 2032), where the whole defect lived in the ONE frame after the edge
/// and the reported spot was a dusk swim off the Savage Coast — a place a screenshot harness
/// cannot get to and a bug a still frame cannot catch. It reproduced in `water-noon` in one
/// command.
fn forced_submersion_frames() -> u32 {
    std::env::var("WOW_FORCE_SUB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// The [`forced_submersion_frames`] override, applied after the real probe so it overwrites a
/// genuine verdict rather than racing it. Prints the release frame, which is the frame every
/// crossing artefact is read against. Registered only when the env names a hold.
fn force_submersion(mut underwater: ResMut<Underwater>, mut frame: Local<u32>) {
    let hold = forced_submersion_frames();
    *frame += 1;
    if *frame <= hold {
        underwater.0 = benilla_formats::Submersion::Water;
    } else if *frame == hold + 1 {
        info!(
            "WOW_FORCE_SUB: frame {} — eye verdict released to Dry",
            *frame
        );
    }
}

/// The frame slot where [`Underwater`] is written — the label every consumer of the submersion
/// verdict orders itself `.after(..)`. The submerged view is a whole-screen swap (atmosphere, clear
/// colour, and the sky-pass suppression all flip on it), so a consumer reading a frame-old verdict
/// shows one mixed frame on every surface crossing — the murk with the sun still up, or clear water
/// with the sky already gone. One label, so the "same frame" contract is written once instead of
/// re-derived per consumer.
#[derive(bevy::ecs::schedule::SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SubmersionVerdict;

/// The water subsystem: load the per-kind frame arrays + shared materials at startup, then cycle the
/// animation frame each update. Spawning the per-chunk surfaces happens in the terrain streamer (via
/// [`spawn_liquids`], water lives *with* its tile), reading [`LiquidAssets`].
pub(crate) struct LiquidPlugin;

impl Plugin for LiquidPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<LiquidMaterial>::default())
            .init_resource::<Underwater>()
            .init_resource::<SubmergedEye>()
            .init_resource::<WaterIndex>()
            // PreUpdate: surfaces stream in/out via Update-side commands, so the edge is visible
            // here the frame after — before any of that frame's consumers ask. A despawn's stale
            // entry in between self-filters at the consumer (`Query::get` misses).
            .add_systems(PreUpdate, maintain_water_index)
            .add_systems(Startup, surface::setup_liquid.after(AssetSet::Open))
            .add_systems(
                Update,
                (
                    // AFTER the portal pass, which publishes the camera eye's room this frame
                    // (`CameraInteriorClaim`): the submersion verdict is the eye's room applied to
                    // the eye's position, and a frame-old room would flash the wrong atmosphere for
                    // one frame on every doorway crossing — which, for a whole-screen filter, is
                    // exactly the artefact that reads worst.
                    query::detect_submersion
                        .after(crate::wmo_portal::WmoPvsSet)
                        .in_set(SubmersionVerdict),
                ),
            )
            // The surface-render kill-switch (see [`hide_liquid_surfaces`]) — inert without the env
            // var, so it costs nothing when it isn't being used.
            //
            // `PostUpdate`, after the two systems that now own a liquid surface's `Visibility`
            // every frame — the exterior-window cull (ADT surfaces, decision 1652) and the
            // model-visibility authority (WMO pools, 0689/0784) — and before Bevy consumes the
            // result. An *override* that runs last, deliberately, rather than a second writer
            // trying to compose with them (decision 0025's law, and 0784's reasoning for why
            // ordering is the wrong tool for two real terms but the right one for a kill-switch:
            // there is nothing to AND here, the switch simply wins).
            .add_systems(
                PostUpdate,
                surface::hide_liquid_surfaces
                    .after(crate::exterior_cull::ExteriorCullSet)
                    .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate)
                    .run_if(|| std::env::var_os("WOW_NO_LIQUID").is_some()),
            );
        // The scripted surfacing crossing (see [`force_submersion`]) — the node is only added
        // when the env asks for it, so an ordinary run carries neither the system nor a per-frame
        // run condition that would re-read the environment to say "no" 60 times a second.
        if forced_submersion_frames() > 0 {
            app.add_systems(
                Update,
                force_submersion
                    .after(query::detect_submersion)
                    .in_set(SubmersionVerdict),
            );
        }
        // The underwater drift cloud — the one thing in this subsystem that RENDERS because the
        // eye is submerged, rather than answering where the liquid is (see the layout note above).
        drift::register(app);
    }
}
