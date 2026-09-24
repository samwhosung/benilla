//! Every animation channel family a model has ([`M2AnimSummary`](benilla_formats::M2AnimSummary))
//! and its texture-transform tracks, for one M2 or each animated M2 matching a substring. A fully
//! static model that changes on screen is being changed by something drawn over it, or by the
//! renderer. Output is Blizzard data: never commit it.
//! `cargo run -p benilla-formats --example dump_model_anim -- <m2 path | substring>`

use std::io::Cursor;

fn main() -> anyhow::Result<()> {
    let arg = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: dump_model_anim <m2 path | substring>"))?;
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;

    // An exact path dumps in full; a substring sweep skips fully static models.
    let exact = chain.contains(&arg);
    let names: Vec<String> = if exact {
        vec![arg]
    } else {
        let pat = arg.to_lowercase();
        chain
            .list()?
            .into_iter()
            .map(|e| e.name)
            .filter(|n| {
                let l = n.to_lowercase();
                l.ends_with(".m2") && l.contains(&pat)
            })
            .collect()
    };

    for name in names {
        let Ok(bytes) = chain.read_file(&name) else {
            continue;
        };
        let Ok(sum) = benilla_formats::parse_m2_animation_summary(&bytes) else {
            continue;
        };
        if !exact && sum.is_fully_static() {
            continue;
        }
        println!("{name}");
        if sum.is_fully_static() {
            println!("    FULLY STATIC — no channel family animates anything");
        }
        println!(
            "    bones {} (seq0 motion {})  gseq {}  alpha {:?}  rgb {:?}  transp {:?}  \
             texanim {}  particles {}  ribbons {}",
            sum.seq0_animated_bone_count,
            sum.seq0_has_bone_motion,
            sum.global_seq_channels.len(),
            sum.color_alpha_tracks,
            sum.color_rgb_tracks,
            sum.transparency_tracks,
            sum.texture_transform_count,
            sum.particle_emitter_count,
            sum.ribbon_emitter_count,
        );
        if sum.texture_transform_count == 0 {
            continue;
        }
        let Ok(fmt) = parse_m2(&bytes) else { continue };
        let m = fmt.model();
        println!(
            "    global_sequences {:?}  texAnimLookup {:?}",
            m.global_sequences, m.texture_transform_lookup
        );
        for (i, t) in m.texture_transforms.iter().enumerate() {
            let tr = &t.translation;
            println!(
                "    tt[{i}] translation: interp {} gseq {:#x} keys {}",
                tr.interp,
                tr.gseq,
                tr.keys.len()
            );
            for (ts, v) in tr.keys.iter().take(8) {
                println!("        t={ts:>7} ms  {v:?}");
            }
            if tr.keys.len() > 8 {
                println!("        … {} more", tr.keys.len() - 8);
            }
        }
    }
    Ok(())
}

fn parse_m2(bytes: &[u8]) -> anyhow::Result<benilla_m2::M2Format> {
    benilla_m2::parse_m2(&mut Cursor::new(bytes)).map_err(|e| anyhow::anyhow!("parsing M2: {e}"))
}
