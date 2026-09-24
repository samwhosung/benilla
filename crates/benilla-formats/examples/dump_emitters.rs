//! Print a model's particle-emitter records — the authored numbers to hold a live
//! `WOW_PARTICLE_CENSUS` line against: `cargo run -p benilla-formats --example dump_emitters --
//! 'World\...\RubyCrystalLarge01.m2'`.
//!
//! The pair is the check that matters: the census says how many particles are
//! *live*, this says how many the file asks for (`rate × lifespan`), and the over-life ramp says
//! how big they get. A mismatch is our sim's bug; a match moves the question to the look.
//!
//! Output is Blizzard data — pipe it to the scratchpad, never into the repo.

fn main() -> anyhow::Result<()> {
    let virt = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: dump_emitters <m2 path>"))?;
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let mut chain = benilla_formats::open_chain(&data)?;
    let bytes = chain.read_file(&virt)?;
    for (i, e) in benilla_formats::parse_m2_particle_emitters(&bytes)?
        .iter()
        .enumerate()
    {
        println!(
            "emitter[{i}] flags {:#x} bone {} shape {:?} blend {:?}",
            e.flags, e.bone, e.shape, e.blend
        );
        println!(
            "  texture {:?} tiles {}x{} head_tail {}",
            e.texture, e.tile_rows, e.tile_cols, e.head_tail
        );
        let now = e.params.sample(None, 0.0, 0.0);
        println!(
            "  speed {} var {} vrange {} hrange {} gravity {} life {} drag {}   (params at t=0)",
            now.emission_speed,
            now.speed_variation,
            now.vertical_range,
            now.horizontal_range,
            now.gravity,
            now.lifespan,
            e.drag
        );
        for (name, slots) in e.params.channel_views() {
            for (s, keys) in slots.iter().enumerate() {
                if let Some(keys) = keys.filter(|k| k.len() > 1) {
                    println!("  ANIMATED {name}/s{s}: {keys:?}");
                }
            }
        }
        match e.timing.constant_rate() {
            Some(r) => println!("  rate {r}/s (constant, every sequence)"),
            None => {
                for (slot, (looping, rate, enabled)) in e.timing.slot_views().iter().enumerate() {
                    println!(
                        "  seq {slot}{}: rate {:?} enabled {:?}",
                        if *looping { "" } else { " (clamped)" },
                        rate,
                        enabled
                    );
                }
            }
        }
        println!(
            "  area {}x{} zsrc {} tail {} spin {}",
            now.area_length, now.area_width, now.z_source, e.tail_time, e.spin
        );
        println!(
            "  twinkle speed {} pct {} min {} max {}",
            e.twinkle_speed, e.twinkle_percent, e.twinkle_min, e.twinkle_max
        );
        println!("  over_life {:?}", e.over_life);
    }
    Ok(())
}
