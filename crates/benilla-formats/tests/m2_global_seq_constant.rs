//! The stowed-weapon attach bones (HumanMale 29 and 30 at the hips, 58 to 62 on the back) are
//! oriented by one key on global sequence 0, itself 0 ms long: a constant every sequence carries.

use benilla_formats::{open_chain, parse_m2_animations};

/// The hip-sheath attach bone (attachment 32) and its authored constant rotation.
const HIP_BONE: u16 = 29;
const HIP_QUAT: [f32; 4] = [0.382, 0.063, -0.922, 0.0];

#[test]
fn stow_attach_bones_carry_their_constant_rotation_in_every_sequence() {
    let data = benilla_formats::wow_data_or_skip!();
    let mut chain = open_chain(&data).expect("open vanilla patch chain");
    let bytes = chain
        .read_file("Character\\Human\\Male\\HumanMale.m2")
        .expect("HumanMale.m2 in the chain");
    let anims = parse_m2_animations(&bytes);
    assert!(!anims.is_empty(), "HumanMale should have sequences");

    for anim in &anims {
        let hip = anim
            .bones
            .iter()
            .find(|bk| bk.bone == HIP_BONE)
            .unwrap_or_else(|| {
                panic!(
                    "anim {}: hip attach bone {HIP_BONE} missing from the keyframe set — the \
                     global-seq constant was dropped again",
                    anim.anim_id
                )
            });
        assert_eq!(
            hip.rotation.len(),
            1,
            "anim {}: expected the single constant rotation key",
            anim.anim_id
        );
        let (t, q) = &hip.rotation[0];
        assert_eq!(*t, 0.0);
        for (a, b) in q.iter().zip(HIP_QUAT) {
            assert!(
                (a - b).abs() < 1e-3,
                "anim {}: hip constant {q:?} != authored {HIP_QUAT:?}",
                anim.anim_id
            );
        }
    }
}
