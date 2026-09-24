//! Which bones carry keys in one sequence, with their first keys; bone indices narrow the list.
//! `cargo run -p benilla-formats --example dump_bone_keys <file.m2> <anim_id> [bone ...]`, the
//! file extracted first with `mpqx`.

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: dump_bone_keys <file.m2> <anim_id> [bone ...]");
    let anim_id: u16 = args.next().expect("anim id").parse().expect("anim id u16");
    let bones: Vec<u16> = args.map(|a| a.parse().expect("bone index u16")).collect();
    let bytes = std::fs::read(&path).expect("read m2");
    if let Some(a) = benilla_formats::parse_m2_string_anchors(&bytes) {
        println!(
            "string anchors: top bone {} ({:+.3},{:+.3},{:+.3}) bottom bone {} ({:+.3},{:+.3},{:+.3})",
            a.top.0, a.top.1[0], a.top.1[1], a.top.1[2], a.bottom.0, a.bottom.1[0], a.bottom.1[1], a.bottom.1[2]
        );
    }
    // anim_id 65535 = list every sequence (id/duration/looping + keyed-bone count only).
    for a in benilla_formats::parse_m2_animations(&bytes) {
        if anim_id == u16::MAX {
            println!(
                "anim {} dur {:.3}s looping {} — {} keyed bones",
                a.anim_id,
                a.duration,
                a.looping,
                a.bones.len()
            );
            continue;
        }
        if a.anim_id != anim_id {
            continue;
        }
        println!(
            "anim {} dur {:.3}s looping {} — {} keyed bones",
            a.anim_id,
            a.duration,
            a.looping,
            a.bones.len()
        );
        for e in &a.events {
            println!(
                "  event {} t={:.3} data {}",
                String::from_utf8_lossy(&e.ident),
                e.time,
                e.data
            );
        }
        for bk in &a.bones {
            if !bones.is_empty() && !bones.contains(&bk.bone) {
                continue;
            }
            println!(
                "  bone {:3}: {} trans, {} rot, {} scale keys",
                bk.bone,
                bk.translation.len(),
                bk.rotation.len(),
                bk.scale.len()
            );
            for (t, q) in bk.rotation.iter().take(3) {
                println!(
                    "    rot t={t:.3} q=({:+.3},{:+.3},{:+.3},{:+.3})",
                    q[0], q[1], q[2], q[3]
                );
            }
            // Translation and scale values, not only counts: a billboard preserves scale per axis.
            for (t, v) in bk.translation.iter().take(8) {
                println!(
                    "    trans t={t:.3} ({:+.3},{:+.3},{:+.3})",
                    v[0], v[1], v[2]
                );
            }
            for (t, v) in bk.scale.iter().take(8) {
                println!(
                    "    scale t={t:.3} ({:+.3},{:+.3},{:+.3})",
                    v[0], v[1], v[2]
                );
            }
        }
    }
}
