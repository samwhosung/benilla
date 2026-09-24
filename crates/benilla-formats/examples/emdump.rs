//! Every particle emitter one model authors, one line each, under the model's reach
//! (`m2_owner_reach`) and the `Transparent3d` rung sized from it (`owner_last_rung`), which keeps
//! its effects drawing after its own transparent batches. `emdump <internal\path.m2>`

fn main() -> anyhow::Result<()> {
    let virt = std::env::args().nth(1).unwrap();
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;
    let bytes = chain.read_file(&virt)?;
    let dir = virt.rsplit_once('\\').map_or("", |(d, _)| d);
    let subs = benilla_formats::parse_m2_render_submeshes(&bytes, dir, &[]).unwrap_or_default();
    // The renderer's own bound and rung, not a re-derivation of them.
    let reach = benilla_formats::m2_owner_reach(&subs);
    let rung = benilla_formats::owner_last_rung(reach);
    println!("{virt}: reach {reach:.3} yd -> draw-order rung {rung:.0}");
    for (i, e) in benilla_formats::parse_m2_particle_emitters(&bytes)?
        .iter()
        .enumerate()
    {
        let now = e.params.sample(None, 0.0, 0.0);
        println!(
            "emitter {i}: bone {:>3} flags {:#06x} model_space={} pos ({:.4},{:.4},{:.4}) blend {:?} \
             shape {:?} speed {:.4}±{:.4} grav {:.4} life {:.4} tex {:?}",
            e.bone,
            e.flags,
            e.model_space(),
            e.position[0],
            e.position[1],
            e.position[2],
            e.blend,
            e.shape,
            now.emission_speed,
            now.speed_variation,
            now.gravity,
            now.lifespan,
            e.texture,
        );
    }
    Ok(())
}
