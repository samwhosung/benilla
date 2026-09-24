//! An M2's authored header box is its animated extent, not its bind pose: `Bird01.m2` is one small
//! bird at the origin whose only sequence flies bone 0 64 yd along X, so a bound taken from the
//! vertices culls the bird while it is still on screen.

use benilla_formats::{open_chain, parse_m2_animations, parse_m2_bounds};

const BIRD: &str = "World\\critter\\birds\\Bird01.m2";

#[test]
fn the_birds_authored_box_covers_a_flight_path_its_bind_pose_never_hints_at() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let bytes = chain.read_file(BIRD).expect("Bird01.m2 in the chain");

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

    // The reference culls on the authored sphere, `rec+0x68` = `bounding_sphere_radius × scale`
    // (`0x682ef0` -> `0x686b80`), about 35 yd around this bird.
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

    // The vertex extent, the box Bevy's `calculate_bounds` derives, is one small bird.
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
    let slack = (0..3).fold(0.0f32, |s, a| {
        s.max(vlo[a] - b.bbox_min[a]).max(b.bbox_max[a] - vhi[a])
    });
    assert!(
        slack > 30.0,
        "the authored box should reach tens of yards past the bind pose, got {slack} yd"
    );
}
