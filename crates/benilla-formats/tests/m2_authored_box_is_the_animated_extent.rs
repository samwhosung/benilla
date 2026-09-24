//! The birds that blink out when you turn the camera.
//!
//! `World\critter\birds\Bird01.m2` is the model that separates a **bind-pose** bound from an
//! **all-animation** one. Its geometry is a single 1.2 × 1.8 × 0.23 yd bird, modelled once at the
//! origin — and its only sequence keys bone 0 with a translation track that flies that bird 64 yd
//! along X and 17 yd along Y, a slow circuit over the treetops. A placed doodad's submesh entity
//! keeps its transform at the placement origin and lets the joint palette move the vertices, so a
//! bound derived from the bind-pose mesh describes a 1-yd box the bird has long since left: turn the
//! camera until that box exits the frustum and the bird vanishes while still on screen.
//!
//! The authored header box is the fix's source, and this pins that it really is the animated extent
//! — off the shipped file, so the premise can't rot. What consumes it (`m2_anim_bound` → the
//! per-submesh `Aabb` of an animated placement) is pinned by `benilla-world`'s own unit tests.
//!
//! Skips (passes) when the client isn't present at `<repo>/WoW/Data`.

use benilla_formats::{open_chain, parse_m2_animations, parse_m2_bounds};

const BIRD: &str = "World\\critter\\birds\\Bird01.m2";

#[test]
fn the_birds_authored_box_covers_a_flight_path_its_bind_pose_never_hints_at() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let bytes = chain.read_file(BIRD).expect("Bird01.m2 in the chain");

    // 1. The mechanism: bone 0 — the root every other bone hangs off — is keyed with a translation
    //    track tens of yards wide. Without this the model would need no special bound at all.
    let anims = parse_m2_animations(&bytes);
    let root = anims
        .iter()
        .flat_map(|a| a.bones.iter())
        .find(|b| b.bone == 0)
        .expect("Bird01 keys bone 0");
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for (_, t) in &root.translation {
        for a in 0..3 {
            lo[a] = lo[a].min(t[a]);
            hi[a] = hi[a].max(t[a]);
        }
    }
    assert!(
        hi[0] - lo[0] > 60.0,
        "bone 0 should fly the bird >60 yd along X, got {}",
        hi[0] - lo[0]
    );

    // 2. The premise: the AUTHORED header box is that flight path, not the bird. Its half-diagonal
    //    is the reference's own cull radius (`rec+0x68` = `bounding_sphere_radius × scale`,
    //    tested at `0x682ef0` -> `0x686b80`) — so a ~35 yd sphere around a 0.6 yd body is what
    //    the real client tests, and it never blinks.
    let b = parse_m2_bounds(&bytes).expect("Bird01 bounds");
    let box_x = b.bbox_max[0] - b.bbox_min[0];
    assert!(
        box_x > 60.0,
        "the authored box should span the whole circuit, got {box_x} yd on X"
    );
    assert!(
        b.sphere_radius > 30.0,
        "the authored sphere radius should be the circuit's, got {}",
        b.sphere_radius
    );

    // 3. …and the bind pose is not remotely it: the vertex extent — the box Bevy's
    //    `calculate_bounds` derives, and what a placed submesh was culled with — is one small bird.
    let model = benilla_m2::parse_m2(&mut std::io::Cursor::new(&bytes[..])).expect("parse Bird01");
    let (mut vlo, mut vhi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in &model.model().vertices {
        for (a, c) in [v.position.x, v.position.y, v.position.z]
            .into_iter()
            .enumerate()
        {
            vlo[a] = vlo[a].min(c);
            vhi[a] = vhi[a].max(c);
        }
    }
    let widest = (0..3).fold(0.0f32, |w, a| w.max(vhi[a] - vlo[a]));
    assert!(
        widest < 3.0,
        "the bind-pose geometry is one small bird, widest axis {widest} yd"
    );
    // The gap IS the bug: the authored box reaches this far past the bind-pose box.
    let slack = (0..3).fold(0.0f32, |s, a| {
        s.max(vlo[a] - b.bbox_min[a]).max(b.bbox_max[a] - vhi[a])
    });
    assert!(
        slack > 30.0,
        "the authored box should reach tens of yards past the bind pose, got {slack} yd"
    );
}
